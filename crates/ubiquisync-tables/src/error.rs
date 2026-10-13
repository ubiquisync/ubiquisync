//! Error type for the tables crate.

use ubiquisync_sql::{
    db::DbError,
    reducer::{PrepareError, RebuildScope},
};

/// An error from the tables layer: either a backend failure or a schema that
/// doesn't match what the table/column IDs require.
#[derive(Debug, thiserror::Error)]
pub enum SchemaSyncError {
    /// A SQL backend error propagated from the [`Db`](ubiquisync_sql::db::Db).
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// A physical table on disk doesn't match the schema its ID implies
    /// (wrong PK shape, missing/mistyped column, missing ts column, …).
    #[error("schema error: {0}")]
    SchemaSync(String),
}

impl SchemaSyncError {
    pub(crate) fn into_prepare(self) -> PrepareError {
        match self {
            SchemaSyncError::Db(e) => PrepareError::Db(e),
            SchemaSyncError::SchemaSync(e) => PrepareError::NeedsRebuild {
                scope: RebuildScope::Container,
                reason: e.to_string(),
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TablesInitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    // TODO when we have a schema schema error in init do we also want to trigger a rebuild?
    #[error("tables error: {0}")]
    SchemaSync(#[from] SchemaSyncError),
    #[error("{0}")]
    InvalidSchema(#[from] InvalidSchemaError),
}

/// A user-declared `TableSchema` is itself invalid — e.g. the number of PK
/// names doesn't match the table ID's PK count, or two of its VIEW columns
/// share a name.
#[derive(Debug, thiserror::Error)]
#[error("invalid schema: {0}")]
pub struct InvalidSchemaError(pub String);
