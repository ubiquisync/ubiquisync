use crate::db::DbType;

/// An error from a SQL backend operation.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A backend error with no more specific variant; carries the backend's
    /// own message.
    #[error("sql error: {0}")]
    Sql(String),
    /// A UNIQUE / PRIMARY KEY conflict. A tracker that keys the op-log on
    /// `(peer_id, entry_idx)` surfaces a re-ingested entry as this — failing the
    /// batch so the whole apply rolls back rather than double-applying, not a
    /// silent skip. Each backend must map its native constraint error to this
    /// variant.
    #[error("unique constraint violation")]
    UniqueViolation,
    /// A column held a value of a different type than the caller requested.
    /// Null in actual is represented as None.
    #[error("type mismatch, expected {expected}, got {actual:?}")]
    TypeMismatch {
        expected: &'static str,
        actual: Option<DbType>,
    },
    /// A `u64` (e.g. a packed HLC timestamp) didn't fit the signed 64-bit
    /// integer a SQL backend stores. On a write the value exceeded `i64::MAX`;
    /// on a read the stored value was negative. Either way it can't round-trip
    /// through a signed column — and the signed `MAX`/`GREATEST` merge guard
    /// would misorder it — so we reject rather than silently wrap. The `i128`
    /// holds the offending value losslessly in both directions.
    #[error("integer value {0} out of range for signed 64-bit storage")]
    IntegerOutOfRange(i128),
    /// A column index past the end of the row was requested.
    #[error("column index {0} out of bounds")]
    ColumnOutOfBounds(usize),
    /// A column was SQL NULL where the caller required a non-null value.
    #[error("unexpected null")]
    UnexpectedNull,
}
