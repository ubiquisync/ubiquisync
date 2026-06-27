use crate::db::{Db, DbValue};
use crate::op::{Delete, Op, Upsert, Value};
use crate::reducer::{Reducer, ReducerError};
use crate::watch::UpsertEvent;
use ubiquisync_core::hlc::Timestamp;
use ubiquisync_sql::db::DbBatch;

impl Reducer {
    pub(crate) async fn sync_delete_schema(
        &mut self,
        db: &dyn Db,
        delete: &Delete,
    ) -> Result<(), Self::Error> {
        self.ensure_table(db, delete.table_id)?;
        Ok(())
    }

    pub(crate) fn apply_delete(
        &mut self,
        db: &mut dyn DbBatch,
        timestamp: Timestamp,
        delete: &Delete,
    ) -> Result<Option<UpsertEvent>, ReducerError> {
        todo!()
    }
}
