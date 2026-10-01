//! Error type for the tables crate.

use ubiquisync_sql::{
    db::DbError,
    reducer::{PrepareError, RebuildScope},
};

/// An error from the tables layer: either a backend failure or a schema that
/// doesn't match what the table/column IDs require.
#[derive(Debug, thiserror::Error)]
pub enum PrepareTablesError {
    /// A SQL backend error propagated from the [`Db`](ubiquisync_sql::db::Db).
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// A physical table on disk doesn't match the schema its ID implies
    /// (wrong PK shape, missing/mistyped column, missing ts column, …).
    #[error("schema error: {0}")]
    SchemaSync(String),
}

impl PrepareTablesError {
    pub(crate) fn to_prepare(self) -> PrepareError {
        match self {
            PrepareTablesError::Db(e) => PrepareError::Db(e),
            PrepareTablesError::SchemaSync(_) => {
                PrepareError::NeedsRebuild(RebuildScope::Container)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("tables error: {0}")]
    Tables(#[from] PrepareTablesError),
    #[error("{0}")]
    InvalidSchema(#[from] InvalidSchemaError),
}

/// A user-declared `TableSchema` is itself invalid — e.g. the number of PK
/// names doesn't match the table ID's PK count, or two of its VIEW columns
/// share a name.
#[derive(Debug, thiserror::Error)]
#[error("invalid schema: {0}")]
pub struct InvalidSchemaError(pub String);
