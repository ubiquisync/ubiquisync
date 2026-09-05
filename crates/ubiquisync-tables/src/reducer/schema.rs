use std::sync::Arc;

use crate::error::TablesError;
use crate::id::TableId;
use crate::physical_schema::PhysicalTableSchema;
use crate::reducer::Reducer;
use tokio::sync::RwLock;
use ubiquisync_sql::db::Db;

impl Reducer {
    /// We must pass in a mutex guard to ensure DDL operations don't happen concurrently.
    pub(crate) async fn ensure_table<'a>(
        &self,
        db: &dyn Db,
        table_id: TableId,
    ) -> Result<Arc<RwLock<PhysicalTableSchema>>, TablesError> {
        {
            let physical_tables = self.physical_tables.read().await;
            if let Some(t) = physical_tables.get(&table_id) {
                return Ok(t.clone());
            }
        }
        let mut physical_tables = self.physical_tables.write().await;
        match physical_tables.entry(table_id) {
            std::collections::hash_map::Entry::Occupied(e) => Ok(e.get().clone()),
            std::collections::hash_map::Entry::Vacant(e) => {
                let table = PhysicalTableSchema::new_surrogate(&self.prefix, table_id, db).await?;
                Ok(e.insert(Arc::new(RwLock::new(table))).clone())
            }
        }
    }
}
