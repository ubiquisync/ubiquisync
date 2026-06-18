mod apply;
mod delete;
mod init;
mod schema;
mod upsert;

use crate::db::DbError;
use crate::id::{ColumnId, TableId};
use crate::schema::TableSchema;
use std::collections::HashMap;

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
