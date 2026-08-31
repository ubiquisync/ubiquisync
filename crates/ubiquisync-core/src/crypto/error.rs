use thiserror::Error;

use crate::codec::ReadError;

#[derive(Error, Debug)]
pub enum CryptoDecodeError {
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("read unknown: {0}")]
    UnknownAlgorithm(u8),
}
