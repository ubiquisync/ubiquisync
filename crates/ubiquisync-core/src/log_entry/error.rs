use thiserror::Error;

use crate::{codec::reader::ReadError, init::Version};

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
    UnknownSignatureAlgorithm(u8),
    #[error("unknown key-exchange algorithm: {0}")]
    UnknownKeyExchangeAlgorithm(u8),
    #[error("unknown hash suite: {0}")]
    UnknownHashSuite(u8),
    #[error("unexpected entry type: {0}")]
    UnexpectedEntryType(u8),
    #[error("invalid server attested user id length: {length}")]
    InvalidServerAttestedId { length: usize },
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
    #[error("unknown init flags: {0}")]
    UnknownInitFlags(u8),
    #[error("unknown init data, {remaining} unreadable bytes, version: {version:?}")]
    UnknownInitData { version: Version, remaining: usize },
}
