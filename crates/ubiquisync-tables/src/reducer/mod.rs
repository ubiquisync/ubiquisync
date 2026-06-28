mod apply;
mod delete;
mod init;
mod schema;
mod upsert;

use crate::id::{ColumnId, TableId};
use crate::op::Op;
use crate::schema::TableSchema;
use crate::watch::ChangeEvent;
use std::collections::HashMap;
use ubiquisync_sql::db::{Db, DbBatch, DbError, DbStatementResult, StmtId};

pub struct Reducer {
    prefix: String,
    table_schemas: HashMap<TableId, TableSchema>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReducerError {
    #[error("db: {0}")]
    Db(#[from] DbError),
    #[error("system table {0:?} not found")]
    TableNotFound(TableId),
    #[error("system column {0:?} not found")]
    ColumnNotFound(ColumnId),
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

#[async_trait::async_trait(?Send)]
impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = ReducerError;
    type ReadState = ();
    type ApplyState = ApplyState;
    type Event = ChangeEvent;

    async fn prepare(&mut self, db: &dyn Db, op: &Op) -> Result<(), Self::Error> {
        match op {
            Op::Upsert(upsert) => self.sync_upsert_schema(db, upsert),
            Op::Delete(delete) => self.sync_delete_schema(db, delete),
        }
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: ubiquisync_core::hlc::Timestamp,
        op: &Op,
        _: &(),
    ) -> Result<ApplyState, Self::Error> {
        match op {
            Op::Upsert(upsert) => self.apply_upsert(batch, timestamp, upsert),
            Op::Delete(delete) => self.apply_delete(batch, timestamp, delete),
        }
    }

    fn post_apply(
        &self,
        apply_state: &Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<ChangeEvent, Self::Error> {
        todo!()
    }
}

pub(crate) struct ApplyState {
    pub stmt_id: StmtId,
    pub staged_event: ChangeEvent,
}
