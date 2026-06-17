/// A Db value — used for both parameters and query results.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
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
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "integer" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_text(&self, idx: usize) -> Result<&str, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "text" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_blob(&self, idx: usize) -> Result<&[u8], DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "blob" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_bool(&self, idx: usize) -> Result<bool, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(*v != 0),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "bool/integer" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_i64(&self, idx: usize) -> Result<Option<i64>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(Some(*v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "integer" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_text(&self, idx: usize) -> Result<Option<&str>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "text" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_uuid(&self, idx: usize) -> Result<[u8; 16], DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                col: idx,
                expected: "16-byte UUID blob",
            }),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "16-byte UUID blob" }),
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
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "16-byte UUID blob" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_blob(&self, idx: usize) -> Result<Option<&[u8]>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch { col: idx, expected: "blob" }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sql error: {0}")]
    Sql(String),
    #[error("type mismatch at column {col}: expected {expected}")]
    TypeMismatch { col: usize, expected: &'static str },
    #[error("column index {0} out of bounds")]
    ColumnOutOfBounds(usize),
    #[error("unexpected null at column {0}")]
    UnexpectedNull(usize),
}


pub trait Db {
    
}