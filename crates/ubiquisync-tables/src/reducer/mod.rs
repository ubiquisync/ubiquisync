mod init;
mod apply;
mod upsert;
mod delete;
mod util;
mod schema;
mod surrogate;

use std::collections::HashMap;
use crate::id::TableId;
use crate::reducer::schema::TableSchema;

pub struct Reducer {
    prefix: String,
    table_schemas: HashMap<TableId, TableSchema>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReducerError {
    #[error("db: {0}")]
    Db(#[from] DbError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("system table {0:?} not found")]
    SysTableNotFound(SysTableId),
    #[error("system column {0:?} not found")]
    SysColumnNotFound(SysColumnId),
    #[error("not implemented: {0}")]
    NotImplemented(String),
}
