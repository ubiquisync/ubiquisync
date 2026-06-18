use crate::db::Db;
use crate::id::TableId;
use crate::reducer::{Reducer, ReducerError};
use crate::schema::TableSchema;

impl Reducer {
    fn ensure_table<'a>(
        &'a mut self,
        db: &dyn Db,
        table_id: TableId,
    ) -> Result<&'a mut TableSchema, ReducerError> {
        if let Some(table) = self.table_schemas.get_mut(&table_id) {
            return Ok(table);
        }

        let table = TableSchema::init_surrogate(&self.prefix, table_id, db)?;
        self.table_schemas.insert(table_id, table);
        Ok(self.table_schemas.get_mut(&table_id).unwrap())
    }
}
