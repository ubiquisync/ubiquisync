use std::marker::PhantomData;

use ubiquisync_core::uuid::Uuid;

use crate::db::{ColType, Cols, DbError, DbValue};

/// One result row: a positional list of column values.
#[derive(Debug)]
pub struct DbRow {
    /// The row's column values, in `SELECT`/column order.
    pub values: Vec<DbValue>,
}

impl DbRow {
    pub(crate) fn get_at<'a, T: ColType>(
        &'a self,
        idx: usize,
    ) -> Result<T::BorrowedType<'a>, DbError> {
        let v = self
            .values
            .get(idx)
            .ok_or(DbError::ColumnOutOfBounds(idx))?;
        T::from_db_val(v)
    }

    /// Read column `idx` as an `i64`. Errors if it is NULL, not an integer, or
    /// out of bounds.
    pub fn get_i64(&self, idx: usize) -> Result<i64, DbError> {
        self.get_at::<i64>(idx)
    }

    /// Read column `idx` as text. Errors if it is NULL, not text, or out of
    /// bounds.
    pub fn get_text(&self, idx: usize) -> Result<&str, DbError> {
        self.get_at::<String>(idx)
    }

    /// Read column `idx` as a byte blob. Errors if it is NULL, not a blob, or
    /// out of bounds.
    pub fn get_blob(&self, idx: usize) -> Result<&[u8], DbError> {
        self.get_at::<Vec<u8>>(idx)
    }

    /// Read a column written via [`DbValue::from_u64`]: a stored integer that
    /// must be non-negative. Mirrors the checked write so a `u64` round-trips
    /// through a signed column without a lossy `as` cast; a negative stored
    /// value (corruption or a hand-edit) is rejected rather than wrapped to a
    /// huge `u64` that would jump the clock to the end of time.
    pub fn get_u64(&self, idx: usize) -> Result<u64, DbError> {
        self.get_at::<u64>(idx)
    }

    /// Read an optional column written via [`DbValue::from_u64`]: a stored integer that
    /// must be non-negative. Mirrors the checked write so a `u64` round-trips
    /// through a signed column without a lossy `as` cast; a negative stored
    /// value (corruption or a hand-edit) is rejected rather than wrapped to a
    /// huge `u64` that would jump the clock to the end of time.
    pub fn get_optional_u64(&self, idx: usize) -> Result<Option<u64>, DbError> {
        self.get_at::<Option<u64>>(idx)
    }

    /// Read column `idx` as an optional `i64`: `None` when NULL. Errors if it
    /// is not an integer or out of bounds.
    pub fn get_optional_i64(&self, idx: usize) -> Result<Option<i64>, DbError> {
        self.get_at::<Option<i64>>(idx)
    }

    /// Read column `idx` as optional text: `None` when NULL. Errors if it is
    /// not text or out of bounds.
    pub fn get_optional_text(&self, idx: usize) -> Result<Option<&str>, DbError> {
        self.get_at::<Option<String>>(idx)
    }

    /// Read column `idx` as an optional byte blob: `None` when NULL. Errors if
    /// it is not a blob or out of bounds.
    pub fn get_optional_blob(&self, idx: usize) -> Result<Option<&[u8]>, DbError> {
        self.get_at::<Option<Vec<u8>>>(idx)
    }

    /// Read column `idx` as a 16-byte UUID, accepting either a native UUID
    /// value or a 16-byte blob. Errors if it is NULL, a wrong-length blob,
    /// another type, or out of bounds.
    pub fn get_uuid(&self, idx: usize) -> Result<[u8; 16], DbError> {
        self.get_at::<Uuid>(idx)
    }

    /// Read column `idx` as an optional 16-byte UUID (native UUID or 16-byte
    /// blob): `None` when NULL. Errors on a wrong-length blob, another type, or
    /// out of bounds.
    pub fn get_optional_uuid(&self, idx: usize) -> Result<Option<[u8; 16]>, DbError> {
        self.get_at::<Option<Uuid>>(idx)
    }
}

pub struct Rows<C: Cols> {
    rows: Vec<DbRow>,
    _phantom: PhantomData<C>,
}

impl<C: Cols> Rows<C> {
    pub fn iter<'a>(&'a self) -> impl Iterator<Item = Result<C::Row<'a>, DbError>> {
        self.rows.iter().map(C::decode)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn first<'a>(&'a self) -> Result<Option<C::Row<'a>>, DbError> {
        match self.iter().next() {
            Some(Ok(r)) => Ok(Some(r)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn one<'a>(&'a self) -> Result<Option<C::Row<'a>>, DbError> {
        if self.len() > 1 {
            return Err(DbError::UnexpectedRowCount { got: self.len() });
        }
        self.first()
    }

    pub fn exactly_one<'a>(&'a self) -> Result<C::Row<'a>, DbError> {
        if self.len() != 1 {
            return Err(DbError::UnexpectedRowCount { got: self.len() });
        }
        C::decode(&self.rows[0])
    }
}

impl<C: Cols> From<Vec<DbRow>> for Rows<C> {
    fn from(rows: Vec<DbRow>) -> Self {
        Self {
            rows,
            _phantom: Default::default(),
        }
    }
}
