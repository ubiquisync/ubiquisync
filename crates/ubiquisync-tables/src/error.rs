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
    /// An op failed validation against the types its table/column IDs imply:
    /// a value whose type doesn't match its column, the wrong number of PK
    /// values, or a column referenced more than once.
    #[error("invalid op: {0}")]
    InvalidOp(String),
    /// A user-declared `TableSchema` is itself invalid — e.g. the number of PK
    /// names doesn't match the table ID's PK count, or two of its VIEW columns
    /// share a name.
    #[error("invalid schema: {0}")]
    InvalidSchema(String),
}
