use std::collections::BTreeMap;

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
    // `name`/`pk_names` name the user-facing VIEW; unread until the VIEW is
    // built (only the physical layer, keyed by IDs, exists today).
    pub(crate) name: String,
    pub(crate) pk_names: Vec<String>,
    pub(crate) value_cols: BTreeMap<ColumnId, ColumnSchema>,
}

#[derive(Debug, Clone)]
pub struct ColumnSchema {
    /// The user-facing column name for the VIEW; unread until the VIEW is built.
    pub name: String,
    pub id: ColumnId,
}

impl TableSchema {
    pub fn new(
        id: TableId,
        name: String,
        pk_names: Vec<String>,
        non_pk_cols: Vec<ColumnSchema>,
    ) -> Self {
        // One VIEW column name per PK slot.
        assert_eq!(
            pk_names.len(),
            id.pk_count(),
            "pk_names must match PK count"
        );
        let value_cols = non_pk_cols.into_iter().map(|col| (col.id, col)).collect();
        Self {
            id,
            name,
            pk_names,
            value_cols,
        }
    }

    pub(crate) async fn create_view(
        &self,
        surrogate_prefix: &str,
        db: &dyn Db,
    ) -> Result<(), TablesError> {
        let id = self.id;
        let surrogate_name = quote_ident(&id.table_name(surrogate_prefix));
        let name = &self.name;

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
        let quoted_name = quote_ident(&name);

        let mut batch = db.new_batch();
        batch.add_statement(&format!("DROP VIEW IF EXISTS {quoted_name}"), &[]);
        batch.add_statement(
            &format!(
                "CREATE VIEW {quoted_name} AS SELECT {} FROM {surrogate_name} \
            WHERE COALESCE({UPSERT_TS_COL}, 0) >= COALESCE({DELETED_TS_COL}, 0)",
                select_clauses.join(", ")
            ),
            &[],
        );
        batch.commit().await?;
        Ok(())
    }
}
