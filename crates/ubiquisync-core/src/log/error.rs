use thiserror::Error;

use crate::{
    codec::{ReadError, WriteError},
    crypto::CryptoDecodeError,
};

#[derive(Error, Debug)]
pub enum LogEncodeError {
    #[error("empty op")]
    EmptyOp,
    #[error("write error: {0}")]
    WriteError(#[from] WriteError),
}

#[derive(Error, Debug)]
pub enum LogDecodeError {
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("u64 add overflow: {0} + {1}")]
    U64AddOverflow(u64, u64),
    #[error("unknown signature algorithm: {0}")]
    UnknownSignatureAlgorithm(u8),
}

impl LogDecodeError {
    pub(crate) fn from_sig_decode_err(err: CryptoDecodeError) -> Self {
        match err {
            CryptoDecodeError::ReadError(e) => LogDecodeError::ReadError(e),
            CryptoDecodeError::UnknownAlgorithm(b) => LogDecodeError::UnknownSignatureAlgorithm(b),
        }
    }
}
