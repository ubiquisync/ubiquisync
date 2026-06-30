#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sql error: {0}")]
    Sql(String),
    /// A UNIQUE / PRIMARY KEY conflict. Op-log ingestion relies on this to
    /// detect an already-ingested `(client_id, client_idx)` and skip the entry;
    /// each backend must map its native constraint error to this variant.
    #[error("unique constraint violation")]
    UniqueViolation,
    #[error("type mismatch at column {col}: expected {expected}")]
    TypeMismatch { col: usize, expected: &'static str },
    #[error("column index {0} out of bounds")]
    ColumnOutOfBounds(usize),
    #[error("unexpected null at column {0}")]
    UnexpectedNull(usize),
}
