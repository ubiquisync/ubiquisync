use std::collections::{BTreeMap, HashSet};

use ubiquisync_sql::{db::Db, util::quote_ident};

use crate::{
    error::TablesError,
    id::{ColumnId, TableId},
    physical_schema::{DELETED_TS_COL, UPSERT_TS_COL},
};

/// TableSchema represents a named table in our schema. It will be exposed for
/// user queries as an SQL VIEW with the provided names. Under the hood, data
/// will be stored in a physical table with surrogate names derived from the table
/// and column IDs.
#[derive(Debug, Clone)]
pub struct TableSchema {
    pub(crate) id: TableId,
    // `name`/`pk_names` name the user-facing VIEW that `create_view` builds over
    // the surrogate physical table.
    pub(crate) name: String,
    pub(crate) pk_names: Vec<String>,
    pub(crate) value_cols: BTreeMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
/// One non-PK column of a [`TableSchema`]: the user-facing name it is exposed
/// under in the VIEW, paired with its type-encoded [`ColumnId`].
pub struct ColumnSchema {
    /// The user-facing column name for the VIEW.
    pub name: String,
    /// The column's type-encoded ID (wire type + index).
    pub id: ColumnId,
}

impl TableSchema {
    /// Build a validated schema for the table `id`, exposed as a VIEW named
    /// `name` with `pk_names` for its key columns and `non_pk_cols` for the
    /// rest. Returns [`TablesError::InvalidSchema`] on an empty/duplicate name,
    /// a PK-name count that doesn't match `id`, or a duplicate column ID.
    pub fn new(
        id: TableId,
        name: String,
        pk_names: Vec<String>,
        non_pk_cols: Vec<ColumnSchema>,
    ) -> Result<Self, TablesError> {
        // A zero-length name quotes to `""`, which Postgres rejects as an empty
        // delimited identifier (SQLite would accept it).
        if name.is_empty() {
            return Err(TablesError::InvalidSchema(
                "table name must not be empty".into(),
            ));
        }

        // One VIEW column name per PK slot.
        if pk_names.len() != id.pk_count() {
            return Err(TablesError::InvalidSchema(format!(
                "table {name:?}: expected {} PK name(s) to match the table ID, got {}",
                id.pk_count(),
                pk_names.len(),
            )));
        }

        // PK + value names become the VIEW's output columns: each must be
        // non-empty and distinct (compared exactly — emitted as quoted idents).
        let mut seen = HashSet::new();
        for col_name in pk_names.iter().chain(non_pk_cols.iter().map(|c| &c.name)) {
            if col_name.is_empty() {
                return Err(TablesError::InvalidSchema(format!(
                    "table {name:?}: column name must not be empty"
                )));
            }
            if !seen.insert(col_name.as_str()) {
                return Err(TablesError::InvalidSchema(format!(
                    "table {name:?}: duplicate column name {col_name:?}"
                )));
            }
        }

        // Each column ID may appear at most once; a repeat would otherwise
        // silently collapse in the map and drop a declared column.
        let mut value_cols = BTreeMap::new();
        for col in non_pk_cols {
            let col_id = col.id;
            if value_cols.insert(col_id, col).is_some() {
                return Err(TablesError::InvalidSchema(format!(
                    "table {name:?}: duplicate column id {}",
                    col_id.col_name()
                )));
            }
        }

        Ok(Self {
            id,
            name,
            pk_names,
            value_cols,
        })
    }

    /// (Re)create the user-facing VIEW over this table's surrogate storage.
    /// `DROP VIEW IF EXISTS` + `CREATE VIEW` makes it idempotent — safe to run
    /// every time a [`Reducer`](crate::reducer) opens. See [`Self::view_sql`].
    pub(crate) async fn create_view(
        &self,
        surrogate_prefix: &str,
        db: &dyn Db,
    ) -> Result<(), TablesError> {
        let (drop_sql, create_sql) = self.view_sql(surrogate_prefix);
        let mut batch = db.new_batch();
        batch.add_statement(&drop_sql, &[]);
        batch.add_statement(&create_sql, &[]);
        batch.commit().await?;
        Ok(())
    }

    /// Build the `(DROP VIEW, CREATE VIEW)` SQL, split from [`Self::create_view`]
    /// so it can be asserted without a database. All identifiers are quoted —
    /// including the surrogate table name, so an uppercase `prefix` isn't
    /// case-folded away by Postgres. The `WHERE` clause hides tombstoned rows
    /// (latest delete newer than latest upsert).
    fn view_sql(&self, surrogate_prefix: &str) -> (String, String) {
        let id = self.id;
        let surrogate_name = quote_ident(&id.table_name(surrogate_prefix));

        let mut select_clauses = vec![];
        for i in 0..id.pk_count() {
            let surrogate_name = id.pk_col_name(i);
            let real_name = quote_ident(&self.pk_names[i]);
            select_clauses.push(format!("{surrogate_name} AS {real_name}"));
        }

        for (col_id, col) in self.value_cols.iter() {
            let surrogate_name = col_id.col_name();
            let real_name = quote_ident(&col.name);
            select_clauses.push(format!("{surrogate_name} AS {real_name}"));
        }

        // TODO in a future PR we should have separate ways of prefixing both the surrogate tables and the view names
        let quoted_name = quote_ident(&self.name);

        let drop_sql = format!("DROP VIEW IF EXISTS {quoted_name}");
        let create_sql = format!(
            "CREATE VIEW {quoted_name} AS SELECT {} FROM {surrogate_name} \
            WHERE COALESCE({UPSERT_TS_COL}, 0) >= COALESCE({DELETED_TS_COL}, 0)",
            select_clauses.join(", ")
        );
        (drop_sql, create_sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::col_type::ColType;

    fn cs(name: &str, id: ColumnId) -> ColumnSchema {
        ColumnSchema {
            name: name.into(),
            id,
        }
    }

    #[test]
    fn rejects_pk_name_count_mismatch() {
        // A single-column PK declared with two names is rejected, not panicked.
        let id = TableId::new(&[ColType::I64], 1);
        let err = TableSchema::new(id, "t".into(), vec!["a".into(), "b".into()], vec![]).unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_duplicate_value_column_name() {
        // Two value columns sharing a user-facing name would make an invalid VIEW.
        let id = TableId::new(&[ColType::I64], 1);
        let (a, b) = (ColumnId::new(0, ColType::Text), ColumnId::new(1, ColType::Text));
        let err = TableSchema::new(id, "t".into(), vec!["id".into()], vec![cs("dup", a), cs("dup", b)])
            .unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_pk_and_value_name_collision() {
        // A PK name colliding with a value-column name is a duplicate VIEW column.
        let id = TableId::new(&[ColType::I64], 1);
        let a = ColumnId::new(0, ColType::Text);
        let err = TableSchema::new(id, "t".into(), vec!["x".into()], vec![cs("x", a)]).unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_duplicate_pk_names() {
        let id = TableId::new(&[ColType::Text, ColType::I64], 1);
        let err = TableSchema::new(id, "t".into(), vec!["k".into(), "k".into()], vec![]).unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_duplicate_column_id() {
        // Same column ID twice (distinct names) would silently collapse in the map.
        let id = TableId::new(&[ColType::I64], 1);
        let a = ColumnId::new(0, ColType::Text);
        let err = TableSchema::new(id, "t".into(), vec!["id".into()], vec![cs("a", a), cs("b", a)])
            .unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_empty_table_name() {
        let id = TableId::new(&[ColType::I64], 1);
        let err = TableSchema::new(id, "".into(), vec!["id".into()], vec![]).unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn rejects_empty_column_name() {
        let id = TableId::new(&[ColType::I64], 1);
        let a = ColumnId::new(0, ColType::Text);
        let err = TableSchema::new(id, "t".into(), vec!["id".into()], vec![cs("", a)]).unwrap_err();
        assert!(matches!(err, TablesError::InvalidSchema(_)), "got {err:?}");
    }

    #[test]
    fn accepts_valid_schema() {
        let id = TableId::new(&[ColType::I64], 1);
        let (a, b) = (ColumnId::new(0, ColType::Text), ColumnId::new(1, ColType::I64));
        let schema =
            TableSchema::new(id, "t".into(), vec!["id".into()], vec![cs("a", a), cs("b", b)]).unwrap();
        assert_eq!(schema.value_cols.len(), 2);
    }

    #[test]
    fn view_sql_quotes_names_and_hides_tombstones() {
        let id = TableId::new(&[ColType::I64], 1);
        let (body, n) = (ColumnId::new(0, ColType::Text), ColumnId::new(1, ColType::I64));
        let schema = TableSchema::new(
            id,
            "widgets".into(),
            vec!["id".into()],
            vec![cs("body", body), cs("n", n)],
        )
        .unwrap();

        let (drop_sql, create_sql) = schema.view_sql("app");

        assert_eq!(drop_sql, r#"DROP VIEW IF EXISTS "widgets""#);
        assert!(
            create_sql.starts_with(r#"CREATE VIEW "widgets" AS SELECT "#),
            "got {create_sql}"
        );
        // The surrogate table name MUST be quoted in FROM (guards the Postgres
        // case-fold fix); a regression to an unquoted name would fail here.
        assert!(create_sql.contains(r#"FROM "app__t0x"#), "got {create_sql}");
        // PK and value columns are aliased from surrogate to declared names.
        assert!(create_sql.contains(r#"k0 AS "id""#), "got {create_sql}");
        assert!(create_sql.contains(r#"AS "body""#), "got {create_sql}");
        assert!(create_sql.contains(r#"AS "n""#), "got {create_sql}");
        // Tombstoned rows (latest delete newer than latest upsert) are hidden.
        assert!(
            create_sql.contains("WHERE COALESCE(__upsert_ts, 0) >= COALESCE(__deleted_ts, 0)"),
            "got {create_sql}"
        );
    }
}
