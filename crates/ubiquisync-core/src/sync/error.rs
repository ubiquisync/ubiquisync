use crate::codec::CodecError;

/// The sync subsystem's umbrella error: anything that can go wrong reading or
/// writing a peer's log ([`LogSource`](super::LogSource) /
/// [`LogEntrySink`](super::LogEntrySink)) or driving entries into a processor
/// ([`PullSync`](super::PullSync)). Storage backends and processors surface
/// their own failures through this type via its `From` conversions.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown peer")]
    UnknownPeer,
    #[error("index out of range")]
    IndexOutOfRange,
    #[error("encoding error: {0}")]
    EncodingError(String),
    #[error("cursor mismatch: expected entry_idx={expected_idx}, got entry_idx={actual_idx}")]
    CursorMismatch { expected_idx: u64, actual_idx: u64 },
    #[error("codec error: {0}")]
    CodecError(#[from] CodecError),
}
