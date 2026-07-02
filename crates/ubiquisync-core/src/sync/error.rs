use crate::codec::CodecError;

/// The sync subsystem's error: reading a log ([`LogSource`](super::LogSource)),
/// applying entries ([`LogProcessor`](super::LogProcessor)), or writing the
/// local log ([`FileLogSink`](super::FileLogSink)). Backends erase their own
/// errors into [`SyncError::Backend`], so the object-safe traits share one fixed
/// error type instead of an associated one.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// An underlying I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A peer id that this source/sink does not know.
    #[error("unknown peer")]
    UnknownPeer,
    /// A requested entry index lay outside the peer's log.
    #[error("index out of range")]
    IndexOutOfRange,
    /// A value could not be encoded for transport/storage.
    #[error("encoding error: {0}")]
    EncodingError(String),
    /// The pull cursor's expected next index disagreed with the source — the
    /// streams are out of sync.
    #[error("cursor mismatch: expected entry_idx={expected_idx}, got entry_idx={actual_idx}")]
    CursorMismatch {
        /// Entry index the cursor expected next.
        expected_idx: u64,
        /// Entry index the source actually presented.
        actual_idx: u64,
    },
    /// A wire-format error decoding a peer's log entries.
    #[error("codec error: {0}")]
    CodecError(#[from] CodecError),
    /// A backend failure erased behind a boxed error, letting a concrete replica
    /// surface its own error type through the fixed trait boundary. Not `Send`:
    /// the traits are `?Send`, matching the `?Send` `Db` backend.
    #[error("backend error: {0}")]
    Backend(Box<dyn std::error::Error>),
}
