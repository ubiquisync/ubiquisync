use thiserror::Error;

use crate::codec::reader::ReadError;

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("empty op")]
    EmptyOp,
    #[error("invalid expunge range")]
    InvalidExpungeRange,
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("unknown cipher suite: {0}")]
    UnknownCipherSuite(u8),
    #[error("unknown signature algorithm: {0}")]
    UknownSignatureAlgorithm(u8),
    #[error("unexpected entry type: {0}")]
    UnexpectedEntryType(u8),
    #[error("invalid server attested user id length: {length}")]
    InvalidServerAttestedId { length: usize },
}
