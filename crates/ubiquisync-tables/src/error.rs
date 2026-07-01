//! Error type for the tables crate.

use ubiquisync_sql::db::DbError;

/// An error from the tables layer: either a backend failure or a schema that
/// doesn't match what the table/column IDs require.
#[derive(Debug, thiserror::Error)]
pub enum TablesError {
    /// A SQL backend error propagated from the [`Db`](ubiquisync_sql::db::Db).
    #[error("db error: {0}")]
    DbError(#[from] DbError),
    /// A physical table on disk doesn't match the schema its ID implies
    /// (wrong PK shape, missing/mistyped column, missing ts column, …).
    #[error("schema error: {0}")]
    SchemaError(String),
}
