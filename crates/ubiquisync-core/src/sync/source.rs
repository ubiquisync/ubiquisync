//! [`LogSource`]: the read side of a replica.

use async_trait::async_trait;

use crate::codec::DecodedEntry;
use crate::sync::cursors::HasCursors;
use crate::uuid::Uuid;

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
pub trait LogSource<E>: HasCursors {
    /// A bounded batch of `peer`'s entries at or after `from`, ascending
    /// (expunged markers included). Empty means drained at `from`; the caller
    /// loops with an advancing `from` until then.
    async fn read_since(
        &self,
        peer: Uuid,
        from: u64,
    ) -> Result<Vec<(u64, DecodedEntry<E>)>, SyncError>;
}
