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
use ubiquisync_sql::db::{Db, DbBatch, DbError};

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

impl ubiquisync_sql::reducer::Reducer for Reducer {
    type Op = Op;
    type Error = ReducerError;
    type Event = ChangeEvent;

    async fn sync_schema(&mut self, db: &dyn Db, op: &Op) -> Result<(), Self::Error> {
        todo!()
    }

    async fn apply(
        &self,
        db: &mut dyn DbBatch,
        timestamp: ubiquisync_core::hlc::Timestamp,
        op: &Op,
    ) -> Result<Self::Event, Self::Error> {
        todo!()
    }
}
