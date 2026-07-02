//! [`LogSource`]: the read side of a replica.

use async_trait::async_trait;

use crate::codec::DecodedEntry;
use crate::uuid::Uuid;

use super::cursors::{CursorStream, PeerCursors};
use super::error::SyncError;

/// The pull side of replication: publish progress as cursors, hand back entries
/// past a position.
///
/// A driver diffs a peer's cursors against its own and pulls only the gap.
/// Keeping the cheap cursor digest separate from the payload avoids a thundering
/// herd — many holders advertise "I have X" for almost nothing, and the receiver
/// fetches X once. `?Send` and object-safe, like
/// [`LogProcessor`](super::LogProcessor).
#[async_trait(?Send)]
pub trait LogSource<E> {
    /// A bounded batch of `peer`'s entries at or after `from`, ascending
    /// (expunged markers included). Empty means drained at `from`; the caller
    /// loops with an advancing `from` until then.
    async fn read_since(
        &self,
        peer: Uuid,
        from: u64,
    ) -> Result<Vec<(u64, DecodedEntry<E>)>, SyncError>;

    /// Snapshot of the current cursor vector, for a one-off diff without a live
    /// subscription.
    async fn cursors(&self) -> Result<PeerCursors, SyncError>;

    /// Live cursor progress: a first [`Snapshot`](super::CursorsEvent::Snapshot),
    /// then [`Advanced`](super::CursorsEvent::Advanced) deltas as cursors move.
    /// Lets a driver react instead of poll; backed by a broadcast (oplog) or a
    /// watch/poll loop (file log).
    fn watch_cursors(&self) -> CursorStream;
}
