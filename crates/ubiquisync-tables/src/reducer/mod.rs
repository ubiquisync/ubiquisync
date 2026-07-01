mod delete;
mod schema;
mod upsert;

use crate::error::TablesError;
use crate::id::TableId;
use crate::op::Op;
use crate::physical_schema::PhysicalTableSchema;
use crate::schema::TableSchema;
use crate::watch::ChangeEvent;
use std::collections::HashMap;
use ubiquisync_sql::db::{Db, DbBatch, DbStatementResult, StmtId};

pub struct Reducer {
    prefix: String,
    all_tables: HashMap<TableId, PhysicalTableSchema>,
    named_tables: HashMap<TableId, TableSchema>,
}

impl Reducer {
    pub async fn new(
        prefix: &str,
        tables: &[TableSchema],
        db: &dyn Db,
    ) -> Result<Self, TablesError> {
        let mut named_tables = HashMap::new();
        let mut all_tables = HashMap::new();
        for table in tables {
            named_tables.insert(table.id, table.clone());
            let physical_table = PhysicalTableSchema::new_named(prefix, table, db).await?;
            all_tables.insert(table.id, physical_table);
        }
        Ok(Self {
            prefix: prefix.into(),
            all_tables,
            named_tables,
        })
    }
}

#[async_trait::async_trait(?Send)]
impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = TablesError;
    type ReadState = ();
    type ApplyState = ApplyState;
    type Event = Option<ChangeEvent>;

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
    ) -> Result<Option<ChangeEvent>, Self::Error> {
        if let Some(event) = apply_state.staged_event {
            match event {
                ChangeEvent::Upsert(event) => {
                    self.post_upsert(apply_state.stmt_id, event, batch_result)
                }
                ChangeEvent::Delete(event) => {
                    self.post_delete(apply_state.stmt_id, event, batch_result)
                }
            }
        } else {
            Ok(None)
        }
    }
}

pub struct ApplyState {
    pub stmt_id: StmtId,
    pub staged_event: Option<ChangeEvent>,
}
