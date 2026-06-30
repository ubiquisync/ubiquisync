use crate::dialect::{PlaceholderGen, SqlDialect};

use super::DbError;

/// A Db value — used for both parameters and query results.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
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
}

#[derive(Debug)]
pub struct DbRow {
    pub values: Vec<DbValue>,
}

impl DbRow {
    pub fn get_i64(&self, idx: usize) -> Result<i64, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(*v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_text(&self, idx: usize) -> Result<&str, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "text",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_blob(&self, idx: usize) -> Result<&[u8], DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    /// Read a column written via [`DbValue::from_u64`]: a stored integer that
    /// must be non-negative. Mirrors the checked write so a `u64` round-trips
    /// through a signed column without a lossy `as` cast; a negative stored
    /// value (corruption or a hand-edit) is rejected rather than wrapped to a
    /// huge `u64` that would jump the clock to the end of time.
    pub fn get_u64(&self, idx: usize) -> Result<u64, DbError> {
        let v = self.get_i64(idx)?;
        u64::try_from(v).map_err(|_| DbError::IntegerOutOfRange(v as i128))
    }

    /// Read an optional column written via [`DbValue::from_u64`]: a stored integer that
    /// must be non-negative. Mirrors the checked write so a `u64` round-trips
    /// through a signed column without a lossy `as` cast; a negative stored
    /// value (corruption or a hand-edit) is rejected rather than wrapped to a
    /// huge `u64` that would jump the clock to the end of time.
    pub fn get_optional_u64(&self, idx: usize) -> Result<Option<u64>, DbError> {
        if let Some(v) = self.get_optional_i64(idx)? {
            Ok(Some(
                u64::try_from(v).map_err(|_| DbError::IntegerOutOfRange(v as i128))?,
            ))
        } else {
            Ok(None)
        }
    }

    pub fn get_bool(&self, idx: usize) -> Result<bool, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(*v != 0),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "bool/integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_i64(&self, idx: usize) -> Result<Option<i64>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(Some(*v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_text(&self, idx: usize) -> Result<Option<&str>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "text",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_uuid(&self, idx: usize) -> Result<[u8; 16], DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                col: idx,
                expected: "16-byte UUID blob",
            }),
            Some(DbValue::Uuid(v)) => Ok(*v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "uuid or 16-byte blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_uuid(&self, idx: usize) -> Result<Option<[u8; 16]>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => {
                let arr = v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                    col: idx,
                    expected: "16-byte UUID blob",
                })?;
                Ok(Some(arr))
            }
            Some(DbValue::Uuid(v)) => Ok(Some(*v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "uuid or 16-byte blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_blob(&self, idx: usize) -> Result<Option<&[u8]>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }
}

pub struct ValueBinder {
    placeholder_gen: PlaceholderGen,
    values: Vec<DbValue>,
}

impl ValueBinder {
    pub fn new(dialect: SqlDialect) -> Self {
        Self {
            placeholder_gen: PlaceholderGen::new(dialect),
            values: vec![],
        }
    }

    pub fn bind_next(&mut self, value: DbValue) -> String {
        self.values.push(value);
        self.placeholder_gen.next_placeholder()
    }

    pub fn values(self) -> Vec<DbValue> {
        self.values
    }
}

#[cfg(test)]
mod tests {
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
}
