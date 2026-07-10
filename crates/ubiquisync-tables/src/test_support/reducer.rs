//! Backend-agnostic suite for the table [`Reducer`]: drives real ops through
//! `prepare` → `apply` → `commit` → `post_apply` against a live `Db`, then reads
//! the physical table back to assert LWW convergence and checks the emitted
//! [`ChangeEvent`].
//!
//! The reducer is driven directly (not via the crate-private `Processor`), so
//! these tests exercise exactly the SQL it builds without pulling in the HLC or
//! tracker. Every scenario uses a distinct table index, so they all share one
//! database without colliding.

use crate::col_type::ColType;
use crate::id::{ColumnId, TableId};
use crate::op::{ColumnSet, Delete, Op, Upsert, Value};
use crate::physical_schema::{DELETED_TS_COL, UPSERT_TS_COL};
use crate::reducer::Reducer;
use crate::schema::{ColumnSchema, TableSchema};
use crate::watch::{ChangeEvent, DeleteEvent, UpsertEvent};
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::{Db, DbValue};
// The reducer's three-phase contract is a trait; bring its methods into scope
// without shadowing the concrete `Reducer` struct above.
use ubiquisync_sql::reducer::Reducer as _;
use ubiquisync_sql::util::quote_ident;

const PREFIX: &str = "app";

/// Runs every reducer scenario against `db`. Call with a freshly opened, empty
/// database.
pub async fn run_reducer_suite<D: Db>(db: D) {
    let db = &db;
    insert_populates_row_and_event(db).await;
    newer_upsert_wins_and_converges(db).await;
    same_timestamp_breaks_tie_by_value(db).await;
    disjoint_columns_merge_independently(db).await;
    delete_tombstones_and_nulls_columns(db).await;
    column_written_after_delete_survives(db).await;
    delete_then_newer_upsert_resurrects(db).await;
    delete_blocks_older_upsert(db).await;
    newer_delete_wins_older_is_silent(db).await;
    event_reports_only_winning_columns(db).await;
    pkey_only_insert_still_emits_event(db).await;
    surrogate_table_emits_no_event(db).await;
    explicit_null_set_merges_by_lww(db).await;
    tiebreak_loser_is_not_reported(db).await;
    rejects_invalid_ops(db).await;
    composite_pk_and_mixed_types(db).await;
    view_exposes_live_rows_under_declared_names(db).await;
    rejects_duplicate_declared_tables(db).await;
}

// ── Scenarios ────────────────────────────────────────────────────────────────

/// A first upsert inserts the row, stamps each column + the table `__upsert_ts`
/// with the op timestamp, and reports every written column as changed.
async fn insert_populates_row_and_event(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 1);
    let (body, n) = (col(0, ColType::Text), col(1, ColType::I64));
    let mut reducer = named(db, id, &[(body, "body"), (n, "n")]).await;

    let ev = apply(
        &mut reducer,
        db,
        100,
        &upsert(id, pk1(1), &[(body, text("hi")), (n, int(7))]),
    )
    .await;

    let up = expect_upsert(ev);
    assert_eq!(up.primary_key, pk1(1));
    assert_eq!(
        changed(&up),
        vec![
            ("body".into(), Some(text("hi"))),
            ("n".into(), Some(int(7))),
        ]
    );

    assert_eq!(read_col(db, id, &pk1(1), body).await, Some((tv("hi"), Some(100))));
    assert_eq!(read_col(db, id, &pk1(1), n).await, Some((iv(7), Some(100))));
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((100, 0)));
}

/// The later write wins regardless of arrival order; the earlier arrival is a
/// no-op that emits no event. Two rows exercise the two orders and must end
/// identical.
async fn newer_upsert_wins_and_converges(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 2);
    let body = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(body, "body")]).await;

    // Row 1: old then new.
    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("old"))])).await;
    let win = apply(&mut reducer, db, 200, &upsert(id, pk1(1), &[(body, text("new"))])).await;
    expect_upsert(win);

    // Row 2: new then old — the old arrival loses and is silent.
    apply(&mut reducer, db, 200, &upsert(id, pk1(2), &[(body, text("new"))])).await;
    let lost = apply(&mut reducer, db, 100, &upsert(id, pk1(2), &[(body, text("old"))])).await;
    expect_none(lost);

    let expected = Some((tv("new"), Some(200)));
    assert_eq!(read_col(db, id, &pk1(1), body).await, expected);
    assert_eq!(read_col(db, id, &pk1(2), body).await, expected);
}

/// Equal timestamps break the tie by comparing the value bytes, so the winner is
/// deterministic no matter the order (`"bbb"` beats `"aaa"`).
async fn same_timestamp_breaks_tie_by_value(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 3);
    let body = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(body, "body")]).await;

    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("aaa"))])).await;
    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("bbb"))])).await;

    apply(&mut reducer, db, 100, &upsert(id, pk1(2), &[(body, text("bbb"))])).await;
    apply(&mut reducer, db, 100, &upsert(id, pk1(2), &[(body, text("aaa"))])).await;

    let winner = Some((tv("bbb"), Some(100)));
    assert_eq!(read_col(db, id, &pk1(1), body).await, winner);
    assert_eq!(read_col(db, id, &pk1(2), body).await, winner);
}

/// Each column carries its own LWW timestamp, so writes to different columns
/// never clobber each other even when they arrive out of timestamp order.
async fn disjoint_columns_merge_independently(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 4);
    let (a, b) = (col(0, ColType::Text), col(1, ColType::I64));
    let mut reducer = named(db, id, &[(a, "a"), (b, "b")]).await;

    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(a, text("x"))])).await;
    apply(&mut reducer, db, 50, &upsert(id, pk1(1), &[(b, int(9))])).await;

    assert_eq!(read_col(db, id, &pk1(1), a).await, Some((tv("x"), Some(100))));
    assert_eq!(read_col(db, id, &pk1(1), b).await, Some((iv(9), Some(50))));
    // __upsert_ts tracks the max of the two upserts.
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((100, 0)));
}

/// A newer delete tombstones the row: it sets `__deleted_ts` and nulls every
/// value column (and its lww) whose write predates the delete.
async fn delete_tombstones_and_nulls_columns(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 5);
    let (a, b) = (col(0, ColType::Text), col(1, ColType::I64));
    let mut reducer = named(db, id, &[(a, "a"), (b, "b")]).await;

    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(a, text("x")), (b, int(9))])).await;
    let ev = apply(&mut reducer, db, 200, &delete(id, pk1(1))).await;
    expect_delete(ev);

    assert_eq!(read_col(db, id, &pk1(1), a).await, Some((DbValue::Null, None)));
    assert_eq!(read_col(db, id, &pk1(1), b).await, Some((DbValue::Null, None)));
    // The row still exists as a tombstone; __upsert_ts is untouched.
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((100, 200)));
}

/// A column written *after* the delete timestamp (its lww is newer) survives the
/// delete's null-out, while an older column is cleared.
async fn column_written_after_delete_survives(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 6);
    let (old, fresh) = (col(0, ColType::Text), col(1, ColType::I64));
    let mut reducer = named(db, id, &[(old, "old"), (fresh, "fresh")]).await;

    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(old, text("x"))])).await;
    apply(&mut reducer, db, 300, &upsert(id, pk1(1), &[(fresh, int(9))])).await;
    apply(&mut reducer, db, 200, &delete(id, pk1(1))).await;

    assert_eq!(read_col(db, id, &pk1(1), old).await, Some((DbValue::Null, None)));
    assert_eq!(read_col(db, id, &pk1(1), fresh).await, Some((iv(9), Some(300))));
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((300, 200)));
}

/// An upsert newer than the delete (`ts >= __deleted_ts`) resurrects the row: it
/// re-populates the column while `__deleted_ts` stays put.
async fn delete_then_newer_upsert_resurrects(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 7);
    let body = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(body, "body")]).await;

    apply(&mut reducer, db, 100, &delete(id, pk1(1))).await;
    let ev = apply(&mut reducer, db, 200, &upsert(id, pk1(1), &[(body, text("back"))])).await;
    expect_upsert(ev);

    assert_eq!(read_col(db, id, &pk1(1), body).await, Some((tv("back"), Some(200))));
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((200, 100)));
}

/// An upsert older than the delete (`ts < __deleted_ts`) is blocked by the
/// delete guard: the row stays a tombstone and no event fires.
async fn delete_blocks_older_upsert(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 8);
    let body = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(body, "body")]).await;

    apply(&mut reducer, db, 200, &delete(id, pk1(1))).await;
    let ev = apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("nope"))])).await;
    expect_none(ev);

    assert_eq!(read_col(db, id, &pk1(1), body).await, Some((DbValue::Null, None)));
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((0, 200)));
}

/// Deletes are LWW too: the newest wins and an older delete is a silent no-op.
async fn newer_delete_wins_older_is_silent(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 9);
    let mut reducer = named(db, id, &[]).await;

    expect_delete(apply(&mut reducer, db, 100, &delete(id, pk1(1))).await);
    expect_delete(apply(&mut reducer, db, 200, &delete(id, pk1(1))).await);
    expect_none(apply(&mut reducer, db, 50, &delete(id, pk1(1))).await);

    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((0, 200)));
}

/// When some columns of an upsert win LWW and others lose, the event lists only
/// the winners — even though the losing column keeps the batch's row count > 0.
async fn event_reports_only_winning_columns(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 10);
    let (a, b) = (col(0, ColType::Text), col(1, ColType::Text));
    let mut reducer = named(db, id, &[(a, "a"), (b, "b")]).await;

    // Seed `b` at a high timestamp.
    apply(&mut reducer, db, 200, &upsert(id, pk1(1), &[(b, text("keep"))])).await;
    // Now write both: `a` is new (wins), `b` is older than the seed (loses).
    let ev = apply(
        &mut reducer,
        db,
        100,
        &upsert(id, pk1(1), &[(a, text("fresh")), (b, text("stale"))]),
    )
    .await;

    let up = expect_upsert(ev);
    assert_eq!(changed(&up), vec![("a".into(), Some(text("fresh")))]);

    assert_eq!(read_col(db, id, &pk1(1), a).await, Some((tv("fresh"), Some(100))));
    assert_eq!(read_col(db, id, &pk1(1), b).await, Some((tv("keep"), Some(200))));
}

/// A pkey-only table has no value columns, so a fresh insert carries no
/// RETURNING row — yet it is a real change and must still emit an event (with an
/// empty column list). An older re-insert is a silent no-op.
async fn pkey_only_insert_still_emits_event(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 11);
    let mut reducer = named(db, id, &[]).await;

    let ev = apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[])).await;
    let up = expect_upsert(ev);
    assert_eq!(up.primary_key, pk1(1));
    assert!(up.changed_columns.is_empty(), "no value columns to report");
    assert_eq!(read_ts(db, id, &pk1(1)).await, Some((100, 0)));

    expect_none(apply(&mut reducer, db, 50, &upsert(id, pk1(1), &[])).await);
}

/// An op against a table the reducer wasn't told about is materialized under a
/// surrogate schema, but produces no change event (there is no user-facing table
/// to name). The data is still persisted, and a later op that references a new
/// column grows the already-created table in place.
async fn surrogate_table_emits_no_event(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 12);
    let (body, extra) = (col(0, ColType::Text), col(1, ColType::I64));
    // No named tables registered.
    let mut reducer = Reducer::new(PREFIX, &[], db).await.unwrap();

    // First op creates the surrogate table with just `body`.
    let ev = apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("x"))])).await;
    expect_none(ev);
    assert_eq!(read_col(db, id, &pk1(1), body).await, Some((tv("x"), Some(100))));

    // A later op references a column the table doesn't have yet: `prepare` must
    // ALTER it in, then the write lands alongside the original column.
    let ev = apply(&mut reducer, db, 200, &upsert(id, pk1(1), &[(extra, int(9))])).await;
    expect_none(ev);
    assert_eq!(read_col(db, id, &pk1(1), extra).await, Some((iv(9), Some(200))));
    assert_eq!(read_col(db, id, &pk1(1), body).await, Some((tv("x"), Some(100))));
}

/// An explicit NULL set merges by LWW like any other value: a newer null clears
/// the column and is reported as a change carrying `None`, while an older null
/// loses to a newer value.
async fn explicit_null_set_merges_by_lww(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 14);
    let a = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(a, "a")]).await;

    // A newer null clears a prior value and reports the column as changed-to-None.
    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(a, text("x"))])).await;
    let up = expect_upsert(apply(&mut reducer, db, 200, &upsert_nulls(id, pk1(1), &[a])).await);
    assert_eq!(changed(&up), vec![("a".into(), None)]);
    assert_eq!(read_col(db, id, &pk1(1), a).await, Some((DbValue::Null, Some(200))));

    // An older null loses to a newer value and is silent.
    apply(&mut reducer, db, 400, &upsert(id, pk1(2), &[(a, text("keep"))])).await;
    expect_none(apply(&mut reducer, db, 300, &upsert_nulls(id, pk1(2), &[a])).await);
    assert_eq!(read_col(db, id, &pk1(2), a).await, Some((tv("keep"), Some(400))));
}

/// A column that loses the same-timestamp value tiebreak must NOT be reported
/// as changed, even when another column in the same op updates the row. The
/// stored value stays the tiebreak winner, and only the genuinely-changed column
/// appears in the event.
async fn tiebreak_loser_is_not_reported(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 15);
    let (a, b) = (col(0, ColType::Text), col(1, ColType::Text));
    let mut reducer = named(db, id, &[(a, "a"), (b, "b")]).await;

    // Seed: `a="zzz"` at ts=100, `b="old"` at ts=50 (so a later `b` will win).
    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(a, text("zzz"))])).await;
    apply(&mut reducer, db, 50, &upsert(id, pk1(1), &[(b, text("old"))])).await;

    // A new op at ts=100 sets `a="aaa"` (SAME ts as stored `a` → value tiebreak,
    // and "aaa" < "zzz" so it loses) and `b="new"` (newer than 50 → wins, so the
    // row IS updated and `a`'s lww still equals 100).
    let ev = apply(
        &mut reducer,
        db,
        100,
        &upsert(id, pk1(1), &[(a, text("aaa")), (b, text("new"))]),
    )
    .await;

    // Only `b` changed; `a` is unchanged despite sharing the timestamp.
    let up = expect_upsert(ev);
    assert_eq!(changed(&up), vec![("b".into(), Some(text("new")))]);
    assert_eq!(read_col(db, id, &pk1(1), a).await, Some((tv("zzz"), Some(100))));
    assert_eq!(read_col(db, id, &pk1(1), b).await, Some((tv("new"), Some(100))));
}

/// Malformed ops are rejected in `prepare` before any schema or SQL work: wrong
/// PK arity or type, a value whose type doesn't match its column, and a column
/// named more than once.
async fn rejects_invalid_ops(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 16);
    let a = col(0, ColType::Text);
    let mut reducer = named(db, id, &[(a, "a")]).await;

    // Wrong PK type (Text where the table declares an I64 PK).
    expect_rejected(&mut reducer, db, &upsert(id, vec![text("x")], &[(a, text("v"))])).await;
    // Wrong PK arity (two values for a single-column PK).
    expect_rejected(&mut reducer, db, &upsert(id, vec![int(1), int(2)], &[(a, text("v"))])).await;
    // Value type doesn't match the column (I64 into a Text column).
    expect_rejected(&mut reducer, db, &upsert(id, pk1(1), &[(a, int(5))])).await;
    // Same column set twice.
    expect_rejected(&mut reducer, db, &upsert(id, pk1(1), &[(a, text("v")), (a, text("w"))])).await;
    // Same column both set and nulled.
    expect_rejected(
        &mut reducer,
        db,
        &Op::Upsert(Upsert {
            table_id: id,
            primary_key: pk1(1),
            sets: vec![ColumnSet {
                column_id: a,
                value: text("v"),
            }],
            nulls: vec![a],
        }),
    )
    .await;
    // Delete with a wrong-type PK.
    expect_rejected(&mut reducer, db, &delete(id, vec![text("x")])).await;

    // Sanity: a well-formed op is still accepted and applies.
    expect_upsert(apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(a, text("ok"))])).await);
}

/// Composite primary keys and every column/PK type round-trip through bind and
/// read-back, and still converge under LWW.
async fn composite_pk_and_mixed_types(db: &dyn Db) {
    let id = TableId::new(&[ColType::Text, ColType::I64], 13);
    let (blob, uuid) = (col(0, ColType::Bytes), col(1, ColType::Uuid));
    let mut reducer = named(db, id, &[(blob, "blob"), (uuid, "uuid")]).await;

    let key = vec![Value::Text("k".into()), Value::I64(42)];
    apply(
        &mut reducer,
        db,
        100,
        &upsert(id, key.clone(), &[(blob, Value::Bytes(vec![1, 2, 3])), (uuid, Value::Uuid([7; 16]))]),
    )
    .await;
    // A newer write to one column wins; the other is untouched.
    apply(&mut reducer, db, 200, &upsert(id, key.clone(), &[(blob, Value::Bytes(vec![9]))])).await;

    assert_eq!(
        read_col(db, id, &key, blob).await,
        Some((DbValue::Blob(vec![9]), Some(200)))
    );
    // A UUID stores natively on Postgres but as a 16-byte blob on SQLite, so
    // normalize to the raw bytes rather than asserting a dialect-specific form.
    let (uuid_val, uuid_lww) = read_col(db, id, &key, uuid).await.unwrap();
    assert_eq!(uuid_lww, Some(100));
    let uuid_bytes = match uuid_val {
        DbValue::Uuid(bytes) => bytes.to_vec(),
        DbValue::Blob(bytes) => bytes,
        other => panic!("unexpected UUID storage: {other:?}"),
    };
    assert_eq!(uuid_bytes, [7u8; 16]);
}

/// The user-facing VIEW created for a named table exposes live rows under the
/// declared table/column names (mapping the surrogate columns back), and hides a
/// row once a newer delete tombstones it — then shows it again after a newer
/// upsert resurrects it. This is the one scenario that reads through the VIEW
/// rather than the physical table.
async fn view_exposes_live_rows_under_declared_names(db: &dyn Db) {
    let id = TableId::new(&[ColType::I64], 17);
    let (body, n) = (col(0, ColType::Text), col(1, ColType::I64));
    // Declared names deliberately differ from the surrogate `k0`/`c0x..` names,
    // so a correct mapping is observable.
    let schema = TableSchema::new(
        id,
        "widgets".into(),
        vec!["widget_id".into()],
        vec![
            ColumnSchema { name: "body".into(), id: body },
            ColumnSchema { name: "n".into(), id: n },
        ],
    )
    .unwrap();
    let mut reducer = Reducer::new(PREFIX, &[schema], db).await.unwrap();

    apply(&mut reducer, db, 100, &upsert(id, pk1(1), &[(body, text("hi")), (n, int(7))])).await;

    // Live row is visible through the VIEW under the declared names.
    let cols = ["widget_id", "body", "n"];
    assert_eq!(
        view_row(db, "widgets", &cols, 1).await,
        Some(vec![iv(1), tv("hi"), iv(7)]),
    );

    // A newer delete tombstones the row; the VIEW's WHERE hides it.
    apply(&mut reducer, db, 200, &delete(id, pk1(1))).await;
    assert_eq!(
        view_row(db, "widgets", &cols, 1).await,
        None,
        "tombstoned row must be hidden by the view",
    );

    // A newer upsert resurrects it; visible again.
    apply(&mut reducer, db, 300, &upsert(id, pk1(1), &[(body, text("back"))])).await;
    assert_eq!(
        view_row(db, "widgets", &["widget_id", "body"], 1).await,
        Some(vec![iv(1), tv("back")]),
    );

    // Tie: an upsert and a delete at the SAME timestamp leave the row visible —
    // `upsert_ts == deleted_ts` satisfies the view's `>=` predicate, and the
    // equal-timestamp column survives the delete's null-out.
    apply(&mut reducer, db, 500, &upsert(id, pk1(2), &[(body, text("tie"))])).await;
    apply(&mut reducer, db, 500, &delete(id, pk1(2))).await;
    assert_eq!(
        view_row(db, "widgets", &["widget_id", "body"], 2).await,
        Some(vec![iv(2), tv("tie")]),
        "row with upsert_ts == deleted_ts must remain visible",
    );
}

/// `Reducer::new` rejects a set of declared tables that repeats a table `id` or
/// a view `name`, before creating anything.
async fn rejects_duplicate_declared_tables(db: &dyn Db) {
    let a = TableId::new(&[ColType::I64], 18);
    let b = TableId::new(&[ColType::I64], 19);

    // Two distinct tables sharing a view name.
    let dup_name = [
        TableSchema::new(a, "dup".into(), vec!["id".into()], vec![]).unwrap(),
        TableSchema::new(b, "dup".into(), vec!["id".into()], vec![]).unwrap(),
    ];
    assert!(
        Reducer::new(PREFIX, &dup_name, db).await.is_err(),
        "duplicate view name must be rejected",
    );

    // The same table id declared twice (under different names).
    let dup_id = [
        TableSchema::new(a, "one".into(), vec!["id".into()], vec![]).unwrap(),
        TableSchema::new(a, "two".into(), vec!["id".into()], vec![]).unwrap(),
    ];
    assert!(
        Reducer::new(PREFIX, &dup_id, db).await.is_err(),
        "duplicate table id must be rejected",
    );
}

/// Read the row with integer PK `k` from the named VIEW, selecting `cols` (the
/// declared column names, `cols[0]` being the PK). Returns the selected values
/// in order, or `None` if no such row is currently visible.
async fn view_row(db: &dyn Db, view: &str, cols: &[&str], k: i64) -> Option<Vec<DbValue>> {
    let select = cols
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {select} FROM {} WHERE {} = {}",
        quote_ident(view),
        quote_ident(cols[0]),
        db.dialect().placeholder(1),
    );
    let rows = db.query(&sql, &[DbValue::Integer(k)]).await.unwrap();
    rows.first().map(|r| r.values.clone())
}

// ── Driving the reducer ──────────────────────────────────────────────────────

/// Run one op end to end: `prepare` (schema reconcile, outside the batch),
/// `apply` (emit statements), `commit`, then `post_apply` (build the event).
async fn apply(reducer: &mut Reducer, db: &dyn Db, raw_ts: u64, op: &Op) -> Option<ChangeEvent> {
    let ts = Timestamp::from_raw(raw_ts);
    reducer.prepare(db, op).await.expect("prepare");
    let mut batch = db.new_batch();
    let state = reducer.apply(batch.as_mut(), ts, op, ()).expect("apply");
    let result = batch.commit().await.expect("commit");
    // A table op emits at most one event; collapse the reducer's 0-or-many `Vec`
    // back to the `Option` these tests assert on, failing loudly if that
    // invariant ever breaks (a duplicate or multi-event regression).
    let mut events = reducer.post_apply(state, &result).expect("post_apply");
    assert!(events.len() <= 1, "a table op must emit at most one event, got {}", events.len());
    events.pop()
}

/// Assert an op is rejected by validation in `prepare`, before any batch runs.
async fn expect_rejected(reducer: &mut Reducer, db: &dyn Db, op: &Op) {
    let result = reducer.prepare(db, op).await;
    assert!(result.is_err(), "expected op to be rejected, got {result:?}");
}

/// A reducer that knows `id` as a named table with the given (column, view-name)
/// value columns.
async fn named(db: &dyn Db, id: TableId, cols: &[(ColumnId, &str)]) -> Reducer {
    let pk_names = (0..id.pk_count()).map(|i| format!("k{i}")).collect();
    let value_cols = cols
        .iter()
        .map(|(c, name)| ColumnSchema {
            name: (*name).into(),
            id: *c,
        })
        .collect();
    let schema = TableSchema::new(id, "t".into(), pk_names, value_cols).unwrap();
    Reducer::new(PREFIX, &[schema], db).await.unwrap()
}

// ── Reading physical state back ──────────────────────────────────────────────

/// `(stored value, lww timestamp)` for `col` of the row keyed by `pk`, or `None`
/// if the row is absent. A present row with a NULL column reads back as
/// `Some((DbValue::Null, None))`.
async fn read_col(
    db: &dyn Db,
    table: TableId,
    pk: &[Value],
    col: ColumnId,
) -> Option<(DbValue, Option<i64>)> {
    let sql = format!(
        "SELECT {}, {} FROM {} WHERE {}",
        col.col_name(),
        col.lww_col_name(),
        quote_ident(&table.table_name(PREFIX)),
        where_pk(db, table),
    );
    let rows = db.query(&sql, &pk_params(pk)).await.unwrap();
    rows.first()
        .map(|r| (r.values[0].clone(), r.get_optional_i64(1).unwrap()))
}

/// `(__upsert_ts, __deleted_ts)` for the row keyed by `pk`, or `None` if absent.
/// Both columns are `NOT NULL DEFAULT 0`, so an unwritten timestamp reads as `0`.
async fn read_ts(db: &dyn Db, table: TableId, pk: &[Value]) -> Option<(i64, i64)> {
    let sql = format!(
        "SELECT {UPSERT_TS_COL}, {DELETED_TS_COL} FROM {} WHERE {}",
        quote_ident(&table.table_name(PREFIX)),
        where_pk(db, table),
    );
    let rows = db.query(&sql, &pk_params(pk)).await.unwrap();
    rows.first()
        .map(|r| (r.get_i64(0).unwrap(), r.get_i64(1).unwrap()))
}

fn where_pk(db: &dyn Db, table: TableId) -> String {
    let dialect = db.dialect();
    (0..table.pk_count())
        .map(|i| format!("{} = {}", table.pk_col_name(i), dialect.placeholder(i + 1)))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn pk_params(pk: &[Value]) -> Vec<DbValue> {
    pk.iter().map(Value::to_db).collect()
}

// ── Builders ─────────────────────────────────────────────────────────────────

fn col(index: u8, ty: ColType) -> ColumnId {
    ColumnId::new(index, ty)
}

fn pk1(k: i64) -> Vec<Value> {
    vec![Value::I64(k)]
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn int(n: i64) -> Value {
    Value::I64(n)
}

/// The `DbValue` a `Text`/`I64` reads back as.
fn tv(s: &str) -> DbValue {
    DbValue::Text(s.into())
}

fn iv(n: i64) -> DbValue {
    DbValue::Integer(n)
}

fn upsert(table: TableId, pk: Vec<Value>, sets: &[(ColumnId, Value)]) -> Op {
    Op::Upsert(Upsert {
        table_id: table,
        primary_key: pk,
        sets: sets
            .iter()
            .map(|(column_id, value)| ColumnSet {
                column_id: *column_id,
                value: value.clone(),
            })
            .collect(),
        nulls: vec![],
    })
}

fn upsert_nulls(table: TableId, pk: Vec<Value>, nulls: &[ColumnId]) -> Op {
    Op::Upsert(Upsert {
        table_id: table,
        primary_key: pk,
        sets: vec![],
        nulls: nulls.to_vec(),
    })
}

fn delete(table: TableId, pk: Vec<Value>) -> Op {
    Op::Delete(Delete {
        table_id: table,
        primary_key: pk,
    })
}

// ── Event assertions ─────────────────────────────────────────────────────────

fn expect_upsert(ev: Option<ChangeEvent>) -> UpsertEvent {
    match ev {
        Some(ChangeEvent::Upsert(u)) => u,
        other => panic!("expected an upsert event, got {other:?}"),
    }
}

fn expect_delete(ev: Option<ChangeEvent>) -> DeleteEvent {
    match ev {
        Some(ChangeEvent::Delete(d)) => d,
        other => panic!("expected a delete event, got {other:?}"),
    }
}

fn expect_none(ev: Option<ChangeEvent>) {
    assert!(ev.is_none(), "expected no event, got {ev:?}");
}

/// The event's changed columns as sorted `(view-name, value)` pairs, for
/// order-insensitive comparison.
fn changed(ev: &UpsertEvent) -> Vec<(String, Option<Value>)> {
    let mut cols: Vec<_> = ev
        .changed_columns
        .iter()
        .map(|c| (c.name.clone(), c.value.clone()))
        .collect();
    cols.sort_by(|a, b| a.0.cmp(&b.0));
    cols
}
