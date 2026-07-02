//! The file-log flavor: single-writer, multi-reader logs on shared storage.
//!
//! A file log is append-only segment directories on cloud storage, one per
//! origin. Only the originating device writes its own log. These traits encode
//! that: a file log reads any origin ([`FileLogPuller`] / [`LogSource`]) but
//! writes only its own ([`FileLogSink`]), so it's a [`FileLogReplica`], never a
//! full [`Replica`](super::Replica). Single-writer files are why dumb file sync
//! suffices and each log's extent is a trustworthy cursor.

use crate::codec::DecodedEntry;
use crate::log_entry::LogEntry;
use crate::uuid::Uuid;

use super::error::SyncError;
use super::source::LogSource;

/// Synchronous reader over the segment directories — one per origin. Sync
/// because it decodes local files; an adapter pairs it with a watch/poll loop to
/// present it as an async [`LogSource`].
pub trait FileLogPuller<E> {
    /// Origins present in this store.
    fn list_peers(&self) -> Vec<Uuid>;

    /// Bounded batch of `peer`'s entries at or after `from`; empty when drained.
    /// The synchronous counterpart to [`LogSource::read_since`].
    fn read_entries(
        &self,
        peer: Uuid,
        from: u64,
    ) -> Result<Vec<(u64, DecodedEntry<E>)>, SyncError>;
}

/// Write side of a file log: append to your own origin only. No method writes a
/// foreign origin — that's the single-writer invariant. The caller supplies each
/// entry's index (the oplog assigns them; the file log follows).
pub trait FileLogSink<E> {
    /// The origin this sink writes — the local node's id.
    fn self_id(&self) -> Uuid;

    /// Append `entries` at their given indices; batched so one call can be one
    /// segment write. Real entries only — expunging rewrites an existing
    /// segment, it never appends.
    fn write(&mut self, entries: &[(u64, LogEntry<E>)]) -> Result<(), SyncError>;
}

/// A file log as a replica: multi-reader ([`LogSource`]), single-writer
/// ([`FileLogSink`]). The single-writer counterpart to
/// [`Replica`](super::Replica); owned by its mirror, not shared.
pub trait FileLogReplica<E>: FileLogSink<E> + LogSource<E> {}

/// Anything that is both is a [`FileLogReplica`].
impl<E, T: FileLogSink<E> + LogSource<E>> FileLogReplica<E> for T {}
