use crate::{
    db::DbType,
    dialect::{PlaceholderGen, SqlDialect},
};

use super::DbError;

/// A Db value — used for both parameters and query results.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    /// SQL NULL.
    Null,
    /// Signed 64-bit integer.
    Integer(i64),
    /// UTF-8 text.
    Text(String),
    /// Raw byte string.
    Blob(Vec<u8>),
    /// 16-byte UUID.
    Uuid([u8; 16]),
}

impl DbValue {
    /// Build an [`Integer`](DbValue::Integer) from a `u64`, erroring when it
    /// exceeds `i64::MAX`. SQL backends store signed 64-bit integers and the
    /// HLC persist guard merges them with a signed `MAX`/`GREATEST`, so a value
    /// past `i64::MAX` could neither round-trip nor order correctly — reject it
    /// rather than silently wrap to a negative. Use this everywhere a `u64`
    /// (packed HLC timestamp, counter, …) is bound into SQL instead of a raw
    /// `as i64` cast. The real clock stays far below the bound (millis below
    /// 2^47, ~year 6400).
    pub fn from_u64(value: u64) -> Result<Self, DbError> {
        i64::try_from(value)
            .map(DbValue::Integer)
            .map_err(|_| DbError::IntegerOutOfRange(value as i128))
    }

    /// Returns the [DbType] of the value or `None`.
    pub fn db_type(&self) -> Option<DbType> {
        match self {
            DbValue::Null => None,
            DbValue::Integer(_) => Some(DbType::Integer),
            DbValue::Text(_) => Some(DbType::Text),
            DbValue::Blob(_) => Some(DbType::Blob),
            DbValue::Uuid(_) => Some(DbType::Uuid),
        }
    }
}

/// Builds a parameterized statement: each [`bind_next`](Self::bind_next)
/// records a value and returns its placeholder string, so a query builder can
/// splice placeholders into SQL text and hand the collected values to the
/// driver in bind order.
pub struct ValueBinder {
    placeholder_gen: PlaceholderGen,
    values: Vec<DbValue>,
}

impl ValueBinder {
    /// Start an empty binder for `dialect`.
    pub fn new(dialect: SqlDialect) -> Self {
        Self {
            placeholder_gen: PlaceholderGen::new(dialect),
            values: vec![],
        }
    }

    /// Record `value` and return its placeholder (`?n` / `$n`) for the SQL text.
    pub fn bind_next(&mut self, value: DbValue) -> String {
        self.values.push(value);
        self.placeholder_gen.next_placeholder()
    }

    /// Consume the binder, returning the bound values in bind order.
    pub fn values(self) -> Vec<DbValue> {
        self.values
    }
}

#[cfg(test)]
mod tests {
    use crate::db::DbRow;

    use super::*;

    #[test]
    fn from_u64_round_trips_through_get_u64() {
        // Values up to i64::MAX bind and read back unchanged — the common case
        // (every realistic HLC timestamp) must be lossless.
        for v in [0u64, 1, 1_700_000_000_000 << 16, i64::MAX as u64] {
            let DbValue::Integer(stored) = DbValue::from_u64(v).unwrap() else {
                panic!("from_u64 must produce an Integer");
            };
            let row = DbRow {
                values: vec![DbValue::Integer(stored)],
            };
            assert_eq!(row.get_u64(0).unwrap(), v);
        }
    }

    #[test]
    fn from_u64_rejects_values_past_i64_max() {
        // A u64 past i64::MAX has no signed representation and would break the
        // signed MAX-guard merge, so it must error rather than wrap negative.
        assert!(matches!(
            DbValue::from_u64(i64::MAX as u64 + 1),
            Err(DbError::IntegerOutOfRange(_))
        ));
        assert!(matches!(
            DbValue::from_u64(u64::MAX),
            Err(DbError::IntegerOutOfRange(_))
        ));
    }

    #[test]
    fn get_u64_rejects_negative_stored_value() {
        // A negative column value (corruption / hand-edit) must not wrap to a
        // huge u64 that jumps the clock forever.
        let row = DbRow {
            values: vec![DbValue::Integer(-1)],
        };
        assert!(matches!(row.get_u64(0), Err(DbError::IntegerOutOfRange(_))));
    }

    #[test]
    fn get_optional_u64_passes_null_through_and_still_guards_range() {
        // NULL is a clean None; a present-but-negative value is still rejected.
        let null_row = DbRow {
            values: vec![DbValue::Null],
        };
        assert_eq!(null_row.get_optional_u64(0).unwrap(), None);

        let value_row = DbRow {
            values: vec![DbValue::Integer(42)],
        };
        assert_eq!(value_row.get_optional_u64(0).unwrap(), Some(42));

        let negative_row = DbRow {
            values: vec![DbValue::Integer(-1)],
        };
        assert!(matches!(
            negative_row.get_optional_u64(0),
            Err(DbError::IntegerOutOfRange(_))
        ));
    }
}
