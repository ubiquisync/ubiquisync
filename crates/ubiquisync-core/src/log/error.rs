use thiserror::Error;

use crate::{
    codec::{ReadError, WriteError},
    crypto::CryptoDecodeError,
};

#[derive(Error, Debug)]
pub enum LogEncodeError {
    #[error("empty op or ops")]
    EmptyOps,
    #[error("write error: {0}")]
    WriteError(#[from] WriteError),
}

#[derive(Error, Debug)]
pub enum LogDecodeError {
    #[error("empty ops")]
    EmptyOps,
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("u64 add overflow: {0} + {1}")]
    U64AddOverflow(u64, u64),
    #[error("unknown signature algorithm: {0}")]
    UnknownSignatureAlgorithm(u8),
    #[error("unhandled entry type: {0}")]
    UndecodableEntryType(u8),
}

#[derive(Error, Debug)]
pub enum LogValidationError {
    #[error("empty op or ops")]
    EmptyOps,
    #[error("invalid timestamp")]
    InvalidTimestamp,
    #[error("invalid server attested user id")]
    InvalidServerAttestedUserId,
}

impl LogDecodeError {
    pub(crate) fn from_sig_decode_err(err: CryptoDecodeError) -> Self {
        match err {
            CryptoDecodeError::ReadError(e) => LogDecodeError::ReadError(e),
            CryptoDecodeError::UnknownAlgorithm(b) => LogDecodeError::UnknownSignatureAlgorithm(b),
        }
    }
}
