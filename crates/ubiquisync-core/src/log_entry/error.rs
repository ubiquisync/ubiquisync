use thiserror::Error;

use crate::{
    codec::reader::ReadError,
    crypto::{Key256Fingerprint, SignatureVerificationError},
    hlc::Timestamp,
};

#[derive(Error, Debug)]
pub enum EncodeError {
    #[error("empty op")]
    EmptyOp,
    #[error("invalid expunge range")]
    InvalidExpungeRange,
}

// #[derive(Error, Debug)]
// pub enum DecodeError {
//     #[error("read error: {0}")]
//     ReadError(#[from] ReadError),
//     #[error("invalid server attested user id length: {length}")]
//     InvalidServerAttestedId { length: usize },
//     #[error("unknown cipher suite: {0}")]
//     UnknownCipherSuite(u8),
//     #[error("unknown signature algorithm: {0}")]
//     UnknownSignatureAlgorithm(u8),
//     #[error("unknown key-exchange algorithm: {0}")]
//     UnknownKeyExchangeAlgorithm(u8),
//     #[error("unknown hash suite: {0}")]
//     UnknownHashSuite(u8),
//     #[error("unexpected entry type: {0}")]
//     UnexpectedEntryType(u8),
//     #[error("unsupported version: {0}")]
//     UnsupportedVersion(u8),
//     #[error("unknown init flags: {0}")]
//     UnknownInitFlags(u8),
//     #[error("unknown init data, {remaining} unreadable bytes, version: {version:?}")]
//     UnknownInitData { version: Version, remaining: usize },
// }

#[derive(Error, Debug)]
pub enum DataIngestError {
    #[error("data corruption error: {0}")]
    DataCorruption(#[from] ReadError),
    #[error("incompatible software version: {0}")]
    Version(#[from] SoftwareVersionError),
    #[error("signature verification failed: {0}")]
    SignatureVerificationFailed(SignatureVerificationError),
}

#[derive(Error, Debug)]
pub enum DataCorruptionError {
    #[error("read error: {0}")]
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
}

pub enum CommitError {
    UnknownOpType(u8),
    CorruptOp,
    OutOfSpace,
    BusyUnreachable,
    NeedReplay,
}
