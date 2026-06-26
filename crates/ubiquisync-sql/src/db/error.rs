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
