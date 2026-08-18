use thiserror::Error;

use crate::{
    codec::reader::ReadError,
    crypto::{Key256Fingerprint, SignatureVerificationError},
    hlc::Timestamp,
};

#[derive(Error, Debug)]
pub enum DataIngestError {
    #[error("data corruption error: {0}")]
    DataCorruption(#[from] DataCorruptionError),
    #[error("incompatible software version: {0}")]
    Version(#[from] SoftwareVersionError),
    #[error("signature verification failed: {0}")]
    SignatureVerificationFailed(SignatureVerificationError),
}

#[derive(Error, Debug)]
pub enum DataCorruptionError {
    #[error("data corruption error: {0}")]
    ReadError(#[from] ReadError),
    #[error("invalid server attested user id length: {length}")]
    InvalidServerAttestedId { length: usize },
}

#[derive(Error, Debug)]
pub enum SoftwareVersionError {
    #[error("unknown signature algorithm: {0}")]
    UnknownSignatureAlgorithm(u8),
    #[error("unknown hash suite: {0}")]
    UnknownHashSuite(u8),
    #[error("unexpected entry type: {0}")]
    UnexpectedEntryType(u8),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
}

#[derive(Error, Debug)]
pub enum LogProcessError {
    #[error("unknown cipher suite: {0}")]
    UnknownCipherSuite(u8),
    #[error("missing decryption key: {0:?}")]
    MissingDecryptionKey(Key256Fingerprint),
    #[error("clock reversal: {last:?} -> {next:?}")]
    ClockReversal { last: Timestamp, next: Timestamp },
    #[error("forward clock skew: {0:? }")]
    ForwardClockSkew(Timestamp /* derive Display to make human readable */),
    #[error("unknown entry type: {0}")]
    UnknownEntryType(u8),
}

pub enum CommitError {
    UnknownOpType(u8),
    OpDecodeError,
    OutOfSpace,
    BusyUnreachable,
    NeedReplay,
}
