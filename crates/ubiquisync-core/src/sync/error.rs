use crate::codec::CodecError;

/// The sync subsystem's umbrella error: anything that can go wrong reading or
/// writing a peer's log ([`LogSource`](super::LogSource) /
/// [`LogEntrySink`](super::LogEntrySink)) or driving entries into a processor
/// ([`PullSynchronizer`](super::PullSynchronizer)). Storage backends and processors surface
/// their own failures through this type via its `From` conversions.
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
}
