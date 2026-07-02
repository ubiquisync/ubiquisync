//! Backend-agnostic physical-schema suite, for driver crates to run against a
//! real [`Db`].
//!
//! Mirrors `ubiquisync-sql`'s `test_support`: the suite is an `async fn` generic
//! over `<D: Db>`, so it's tied to no particular executor. A driver crate (e.g.
//! `ubiquisync-sqlite`) hands it a freshly opened database and drives it with
//! its own `block_on`.
//!
//! Every case uses a distinct table index, so they can all run against one
//! database without colliding.

use std::collections::BTreeSet;

use ubiquisync_sql::db::{Db, DbColumnDescription, DbTableDescriptor, DbType};
use ubiquisync_sql::dialect::SqlDialect;
use ubiquisync_sql::util::quote_ident;

use crate::{
    col_type::ColType,
    id::{ColumnId, TableId},
    physical_schema::{DELETED_TS_COL, PhysicalTableSchema, UPSERT_TS_COL},
    schema::{ColumnSchema, TableSchema},
};

const PREFIX: &str = "app";

fn col_id(index: u8, ty: ColType) -> ColumnId {
    ColumnId::new(index, ty)
}

fn col_schema(index: u8, ty: ColType, name: &str) -> ColumnSchema {
    ColumnSchema {
        name: name.into(),
        id: col_id(index, ty),
    }
}

/// The concrete storage class `describe_table` reports for a column of type
/// `ct` on `dialect`. We need the concrete type (not [`ColType::accepts`]) to
/// build a golden descriptor for an equality check. The only dialect-dependent
/// case is `Uuid`: Postgres stores it in its native `UUID` type, while SQLite
/// has none and falls back to a raw `Blob`.
fn stored_db_type(dialect: SqlDialect, ct: ColType) -> DbType {
    match (dialect, ct) {
        (_, ColType::Text) => DbType::Text,
        (_, ColType::I64) => DbType::Integer,
        (_, ColType::Bytes) => DbType::Blob,
        (SqlDialect::Sqlite, ColType::Uuid) => DbType::Blob,
        (SqlDialect::Postgres, ColType::Uuid) => DbType::Uuid,
    }
}

fn col_desc(name: String, db_type: DbType, nullable: bool) -> DbColumnDescription {
    DbColumnDescription {
        name,
        db_type,
        nullable,
    }
}

/// The descriptor `create_table` should have produced for `id` with value
/// columns `cols`: a `NOT NULL` PK column per slot, the two nullable ts columns,
/// and a nullable value + nullable-integer lww column per value column. Non-PK
/// columns are sorted by name so the comparison is order-insensitive.
fn expected_descriptor(
    dialect: SqlDialect,
    id: TableId,
    cols: &[(ColumnId, ColType)],
) -> DbTableDescriptor {
    let pk_cols = (0..id.pk_count())
        .map(|i| {
            col_desc(
                id.pk_col_name(i),
                stored_db_type(dialect, id.pk_col_type(i)),
                false,
            )
        })
        .collect();

    let mut non_pk = vec![
        col_desc(UPSERT_TS_COL.into(), DbType::Integer, true),
        col_desc(DELETED_TS_COL.into(), DbType::Integer, true),
    ];
    for (cid, ct) in cols {
        non_pk.push(col_desc(cid.col_name(), stored_db_type(dialect, *ct), true));
        non_pk.push(col_desc(cid.lww_col_name(), DbType::Integer, true));
    }
    non_pk.sort_by(|a, b| a.name.cmp(&b.name));

    DbTableDescriptor {
        name: id.table_name(PREFIX),
        pk_cols,
        cols: non_pk,
    }
}

/// Read the physical table for `id` and assert it exactly matches the layout
/// implied by `id` + `cols` — no stray columns, right types, right nullability.
async fn assert_table(db: &dyn Db, id: TableId, cols: &[(ColumnId, ColType)]) {
    let name = id.table_name(PREFIX);
    let mut actual = db
        .describe_table(&name)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("table {name} should exist"));
    actual.cols.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(actual, expected_descriptor(db.dialect(), id, cols));
}

/// Runs every physical-schema scenario against `db`. Call with a freshly opened,
/// empty database.
pub async fn run_physical_schema_suite<D: Db>(db: D) {
    named_tables_create_expected_layouts(&db).await;
    named_table_adds_missing_columns(&db).await;
    surrogate_table_created_bare(&db).await;
    surrogate_table_reconstructs_existing(&db).await;
    ensure_column_adds_pair_and_is_idempotent(&db).await;
    reconciliation_rejects_malformed(db.dialect());
}

/// A few different declared schemas each produce the physical layout their
/// table/column IDs imply.
async fn named_tables_create_expected_layouts(db: &dyn Db) {
    // 1) Single UUID PK, a Text and an I64 value column.
    let id1 = TableId::new(&[ColType::Uuid], 1);
    let schema1 = TableSchema::new(
        id1,
        "notes".into(),
        vec!["id".into()],
        vec![
            col_schema(0, ColType::Text, "body"),
            col_schema(1, ColType::I64, "n"),
        ],
    )
    .unwrap();
    let phys = PhysicalTableSchema::new_named(PREFIX, &schema1, db)
        .await
        .unwrap();
    assert_eq!(phys.get_quoted_name(), quote_ident(&id1.table_name(PREFIX)));
    assert_table(
        db,
        id1,
        &[
            (col_id(0, ColType::Text), ColType::Text),
            (col_id(1, ColType::I64), ColType::I64),
        ],
    )
    .await;

    // 2) Composite (Text, I64) PK, a single Bytes value column.
    let id2 = TableId::new(&[ColType::Text, ColType::I64], 2);
    let schema2 = TableSchema::new(
        id2,
        "events".into(),
        vec!["k".into(), "seq".into()],
        vec![col_schema(0, ColType::Bytes, "payload")],
    )
    .unwrap();
    PhysicalTableSchema::new_named(PREFIX, &schema2, db)
        .await
        .unwrap();
    assert_table(db, id2, &[(col_id(0, ColType::Bytes), ColType::Bytes)]).await;

    // 3) Single I64 PK, no value columns at all.
    let id3 = TableId::new(&[ColType::I64], 3);
    let schema3 = TableSchema::new(id3, "counters".into(), vec!["id".into()], vec![]).unwrap();
    PhysicalTableSchema::new_named(PREFIX, &schema3, db)
        .await
        .unwrap();
    assert_table(db, id3, &[]).await;
}

/// Declaring a wider schema over an already-created table ALTERs in the newly
/// declared columns and leaves the existing one untouched.
async fn named_table_adds_missing_columns(db: &dyn Db) {
    let id = TableId::new(&[ColType::Uuid], 7);

    let v1 = TableSchema::new(
        id,
        "t".into(),
        vec!["id".into()],
        vec![col_schema(0, ColType::Text, "a")],
    )
    .unwrap();
    PhysicalTableSchema::new_named(PREFIX, &v1, db)
        .await
        .unwrap();
    assert_table(db, id, &[(col_id(0, ColType::Text), ColType::Text)]).await;

    // The wider schema is the first one plus two more columns.
    let mut v2 = v1.clone();
    v2.value_cols
        .insert(col_id(2, ColType::I64), col_schema(2, ColType::I64, "b"));
    v2.value_cols
        .insert(col_id(3, ColType::Bytes), col_schema(3, ColType::Bytes, "c"));

    let phys = PhysicalTableSchema::new_named(PREFIX, &v2, db)
        .await
        .unwrap();

    let expected = [
        (col_id(0, ColType::Text), ColType::Text),
        (col_id(2, ColType::I64), ColType::I64),
        (col_id(3, ColType::Bytes), ColType::Bytes),
    ];
    assert_table(db, id, &expected).await;
    let want: BTreeSet<ColumnId> = expected.iter().map(|(c, _)| *c).collect();
    assert_eq!(phys.col_ids(), &want, "schema tracks all declared columns");
}

/// A brand-new surrogate table (known only by ID) is created with just its PK
/// and ts columns — no value columns yet.
async fn surrogate_table_created_bare(db: &dyn Db) {
    let id = TableId::new(&[ColType::Uuid], 9);

    let phys = PhysicalTableSchema::new_surrogate(PREFIX, id, db)
        .await
        .unwrap();
    assert_eq!(phys.get_quoted_name(), quote_ident(&id.table_name(PREFIX)));
    assert!(
        phys.col_ids().is_empty(),
        "bare surrogate has no value columns"
    );
    assert_table(db, id, &[]).await;
}

/// Opening an existing table purely by ID reconstructs its value columns from
/// the database (the version-skew path).
async fn surrogate_table_reconstructs_existing(db: &dyn Db) {
    let id = TableId::new(&[ColType::Text, ColType::Uuid], 4);

    let declared = TableSchema::new(
        id,
        "t".into(),
        vec!["k".into(), "u".into()],
        vec![
            col_schema(1, ColType::I64, "x"),
            col_schema(5, ColType::Text, "y"),
        ],
    )
    .unwrap();
    PhysicalTableSchema::new_named(PREFIX, &declared, db)
        .await
        .unwrap();

    let phys = PhysicalTableSchema::new_surrogate(PREFIX, id, db)
        .await
        .unwrap();
    assert_eq!(phys.get_quoted_name(), quote_ident(&id.table_name(PREFIX)));
    let want: BTreeSet<ColumnId> = [col_id(1, ColType::I64), col_id(5, ColType::Text)].into();
    assert_eq!(phys.col_ids(), &want, "reconstructed value columns");
    assert_table(
        db,
        id,
        &[
            (col_id(1, ColType::I64), ColType::I64),
            (col_id(5, ColType::Text), ColType::Text),
        ],
    )
    .await;
}

/// `ensure_column` adds the value+lww pair, is a no-op the second time, and
/// supports adding further columns of different types.
async fn ensure_column_adds_pair_and_is_idempotent(db: &dyn Db) {
    let id = TableId::new(&[ColType::Uuid], 12);

    let mut phys = PhysicalTableSchema::new_surrogate(PREFIX, id, db)
        .await
        .unwrap();
    assert!(phys.col_ids().is_empty());

    let c1 = col_id(2, ColType::Text);
    phys.ensure_column(db, c1).await.unwrap();
    assert!(phys.col_ids().contains(&c1));
    assert_table(db, id, &[(c1, ColType::Text)]).await;

    // Idempotent: re-adding must not error or change the table.
    let before = db.describe_table(&id.table_name(PREFIX)).await.unwrap();
    phys.ensure_column(db, c1).await.unwrap();
    let after = db.describe_table(&id.table_name(PREFIX)).await.unwrap();
    assert_eq!(before, after, "re-adding a column is a no-op");

    let c2 = col_id(7, ColType::Bytes);
    phys.ensure_column(db, c2).await.unwrap();
    assert_table(db, id, &[(c1, ColType::Text), (c2, ColType::Bytes)]).await;
}

/// Reconciliation rejects every way an on-disk table can fail to match the shape
/// its ID implies. Each case starts from a valid descriptor and corrupts exactly
/// one thing, so a check that silently stopped firing would surface here. This
/// drives the validator directly (no `Db`), so it's dialect-pure apart from the
/// baseline's stored types.
fn reconciliation_rejects_malformed(dialect: SqlDialect) {
    // Baseline: 1 UUID PK, a single Text value column.
    let id = TableId::new(&[ColType::Uuid], 20);
    let value = col_id(0, ColType::Text);
    let valid = expected_descriptor(dialect, id, &[(value, ColType::Text)]);
    let (col, lww) = (value.col_name(), value.lww_col_name());

    // The untouched baseline must reconcile — otherwise the cases below prove
    // nothing.
    PhysicalTableSchema::new_from_db_descriptor(PREFIX, valid.clone())
        .expect("baseline descriptor should reconcile");

    fn col_mut<'a>(d: &'a mut DbTableDescriptor, name: &str) -> &'a mut DbColumnDescription {
        d.cols
            .iter_mut()
            .find(|c| c.name == name)
            .expect("column present")
    }

    macro_rules! rejects {
        ($label:expr, |$d:ident| $body:expr) => {{
            let mut owned = valid.clone();
            {
                let $d = &mut owned;
                $body;
            }
            assert!(
                PhysicalTableSchema::new_from_db_descriptor(PREFIX, owned).is_err(),
                "{} should be rejected",
                $label
            );
        }};
    }

    // Table name that doesn't decode to a TableId.
    rejects!("unparseable table name", |d| d.name = "not_a_table".into());

    // Primary-key defects.
    rejects!("too few PK columns", |d| { d.pk_cols.pop(); });
    rejects!("extra PK column", |d| d
        .pk_cols
        .push(col_desc("k1".into(), DbType::Integer, false)));
    rejects!("PK wrong type", |d| d.pk_cols[0].db_type = DbType::Text);
    rejects!("PK nullable", |d| d.pk_cols[0].nullable = true);
    rejects!("PK wrong name", |d| d.pk_cols[0].name = "x0".into());

    // Bookkeeping-timestamp defects.
    rejects!("missing __upsert_ts", |d| d.cols.retain(|c| c.name != UPSERT_TS_COL));
    rejects!("missing __deleted_ts", |d| d.cols.retain(|c| c.name != DELETED_TS_COL));
    rejects!("__deleted_ts wrong type", |d| col_mut(d, DELETED_TS_COL).db_type = DbType::Text);

    // Value / lww column defects.
    rejects!("value column wrong type", |d| col_mut(d, &col).db_type = DbType::Integer);
    rejects!("value column not nullable", |d| col_mut(d, &col).nullable = false);
    rejects!("missing lww partner", |d| d.cols.retain(|c| c.name != lww));
    rejects!("lww wrong type", |d| col_mut(d, &lww).db_type = DbType::Text);
    rejects!("lww not nullable", |d| col_mut(d, &lww).nullable = false);
}
