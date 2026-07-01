mod apply;
mod delete;
mod schema;
mod upsert;

use crate::error::TablesError;
use crate::id::{ColumnId, TableId};
use crate::op::Op;
use crate::physical_schema::PhysicalTableSchema;
use crate::schema::TableSchema;
use crate::watch::ChangeEvent;
use std::collections::HashMap;
use ubiquisync_sql::db::{Db, DbBatch, DbError, DbStatementResult, StmtId};

pub struct Reducer {
    prefix: String,
    all_tables: HashMap<TableId, PhysicalTableSchema>,
    named_tables: HashMap<TableId, TableSchema>,
}

#[async_trait::async_trait(?Send)]
impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = TablesError;
    type ReadState = ();
    type ApplyState = ApplyState;
    type Event = ChangeEvent;

    async fn prepare(&mut self, db: &dyn Db, op: &Op) -> Result<(), Self::Error> {
        match op {
            Op::Upsert(upsert) => self.sync_upsert_schema(db, upsert).await?,
            Op::Delete(delete) => self.sync_delete_schema(db, delete).await?,
        };
        Ok(())
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: ubiquisync_core::hlc::Timestamp,
        op: &Op,
        _: (),
    ) -> Result<ApplyState, Self::Error> {
        match op {
            Op::Upsert(upsert) => self.apply_upsert(batch, timestamp, upsert),
            Op::Delete(delete) => self.apply_delete(batch, timestamp, delete),
        }
    }

    fn post_apply(
        &self,
        apply_state: Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<ChangeEvent, Self::Error> {
        todo!()
    }
}

pub(crate) struct ApplyState {
    pub stmt_id: StmtId,
    pub staged_event: Option<ChangeEvent>,
}
