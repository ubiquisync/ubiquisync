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
    /// A `u64` (e.g. a packed HLC timestamp) didn't fit the signed 64-bit
    /// integer a SQL backend stores. On a write the value exceeded `i64::MAX`;
    /// on a read the stored value was negative. Either way it can't round-trip
    /// through a signed column — and the signed `MAX`/`GREATEST` merge guard
    /// would misorder it — so we reject rather than silently wrap. The `i128`
    /// holds the offending value losslessly in both directions.
    #[error("integer value {0} out of range for signed 64-bit storage")]
    IntegerOutOfRange(i128),
    #[error("column index {0} out of bounds")]
    ColumnOutOfBounds(usize),
    #[error("unexpected null at column {0}")]
    UnexpectedNull(usize),
}
