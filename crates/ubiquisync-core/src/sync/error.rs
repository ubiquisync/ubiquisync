use crate::codec::CodecError;

/// Errors returned by log store operations.
#[derive(Debug, thiserror::Error)]
pub enum LogStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown peer")]
    UnknownPeer,
    #[error("index out of range")]
    IndexOutOfRange,
    #[error("unexpected origin: {0}")]
    UnexpectedOrigin(String),
    #[error("encoding error: {0}")]
    EncodingError(String),
    #[error("cursor mismatch: expected entry_idx={expected_idx}, got entry_idx={actual_idx}")]
    CursorMismatch { expected_idx: u64, actual_idx: u64 },
    #[error("codec error: {0}")]
    CodecError(#[from] CodecError),
}
