use thiserror::Error;
use ubiquisync_core::{
    crypto::SigningError,
    log::{SegmentCipherError, segment::SegmentEncodeError},
    uuid::Uuid,
};

use crate::{
    db::{DbError, DbRow, DbValue},
    op::OpEncodeError,
};

#[async_trait::async_trait]
pub trait SqlQueryStore {
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError>;
    fn dialect(&self) -> crate::dialect::SqlDialect;
}

#[async_trait::async_trait]
pub trait Exec<Op> {
    async fn exec(&self, server_user_id: Option<Uuid>, op: Op) -> Result<(), ExecError>;
}

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum ExecError {
    #[error("reducer error: {0}")]
    Reducer(BoxError),
    #[error("unexpected error: {0}")]
    Internal(String),
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("segment cipher error: {0}")]
    SegmentCipher(#[from] SegmentCipherError),
    #[error("signing error: {0}")]
    SigningError(#[from] SigningError),
    #[error("segment encode error: {0}")]
    SegmentEncode(#[from] SegmentEncodeError),
    #[error("op encode error: {0}")]
    OpEncode(#[from] OpEncodeError),
}
