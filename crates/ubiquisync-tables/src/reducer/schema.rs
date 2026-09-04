use crate::error::TablesError;
use crate::id::TableId;
use crate::physical_schema::PhysicalTableSchema;
use crate::reducer::Reducer;
use futures::lock::MutexGuard;
use ubiquisync_sql::db::Db;

impl Reducer {
    /// We must pass in a mutex guard to ensure DDL operations don't happen concurrently.
    pub(crate) async fn ensure_table<'a>(
        &self,
        _guard: &MutexGuard<'_, ()>,
        db: &dyn Db,
        table_id: TableId,
    ) -> Result<PhysicalTableSchema, TablesError> {
        if let Some(t) = self.get_table(table_id) {
            Ok(t)
        } else {
            let table = PhysicalTableSchema::new_surrogate(&self.prefix, table_id, db).await?;
            self.set_table(table_id, table.clone());
            Ok(table)
        }
    }

    pub(crate) fn get_table(&self, table_id: TableId) -> Option<PhysicalTableSchema> {
        let physical_tables = self
            .physical_tables
            .read()
            .unwrap_or_else(|e| e.into_inner());
        physical_tables.get(&table_id).cloned()
    }

    pub(crate) fn set_table(&self, table_id: TableId, schema: PhysicalTableSchema) {
        let mut physical_tables = self
            .physical_tables
            .write()
            .unwrap_or_else(|e| e.into_inner());
        physical_tables.insert(table_id, schema);
    }

    pub(crate) fn require_table(
        &self,
        table_id: TableId,
    ) -> Result<PhysicalTableSchema, TablesError> {
        self.get_table(table_id)
            .ok_or(TablesError::SchemaError(format!(
                "table not found: {:?}",
                table_id,
            )))
    }
}
