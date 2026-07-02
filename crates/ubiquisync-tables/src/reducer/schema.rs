use crate::error::TablesError;
use crate::id::TableId;
use crate::physical_schema::PhysicalTableSchema;
use crate::reducer::Reducer;
use std::collections::hash_map::Entry;
use ubiquisync_sql::db::Db;

impl Reducer {
    pub(crate) async fn ensure_table(
        &mut self,
        db: &dyn Db,
        table_id: TableId,
    ) -> Result<&mut PhysicalTableSchema, TablesError> {
        match self.all_tables.entry(table_id) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => {
                // init_surrogate borrows `self.prefix`, a field disjoint from
                // `self.table_schemas`, so the borrow checker allows it here.
                let table = PhysicalTableSchema::new_surrogate(&self.prefix, table_id, db).await?;
                Ok(e.insert(table))
            }
        }
    }

    pub(crate) fn require_table(
        &self,
        table_id: TableId,
    ) -> Result<&PhysicalTableSchema, TablesError> {
        self.all_tables
            .get(&table_id)
            .ok_or(TablesError::SchemaError(format!(
                "table not found: {:?}",
                table_id,
            )))
    }
}
