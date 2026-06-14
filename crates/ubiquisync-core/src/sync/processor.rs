//! The apply seam: [`LogProcessor`] is what the sync engine drives. A
//! processor — typically a materialized store — absorbs remote entries and
//! remembers how far it has read each peer's stream.

use crate::log_entry::LogEntry;
use crate::uuid::Uuid;

use super::error::SyncError;

/// A store that can absorb remote entries and remember how far it has read each
/// peer's stream.
///
/// # Crash & retry contract
///
/// [`PullSynchronizer`](super::PullSynchronizer) applies a peer's batch entry-by-entry and saves
/// the cursor once, *after* the batch. The cursor save and the applies are
/// separate fallible steps with no transaction spanning them — so if a save
/// fails, or the process dies between the last apply and the save, the next sync
/// re-delivers entries that were already applied. Implementors must therefore
/// guarantee **one** of:
///
/// - `apply_remote_entry` is idempotent in `(peer, index)` — re-applying an
///   already-applied entry is a no-op; or
/// - each `apply_remote_entry` + the subsequent `save_peer_cursor` are committed
///   atomically (e.g. one DB transaction per entry, cursor advanced in lockstep).
///
/// Without one of these, a mid-batch failure double-applies entries.
pub trait LogProcessor<E> {
    type Error: From<SyncError>;

    /// Returns the next-entry index for the given peer; `0` if the peer has
    /// never been seen.
    fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error>;

    /// Persists the next-entry index for the given peer.
    fn save_peer_cursor(&mut self, peer_id: &Uuid, next_entry_idx: u64) -> Result<(), Self::Error>;

    /// Applies one remote entry, identified by its stream index. Must honor the
    /// idempotency/atomicity contract described on the trait.
    fn apply_remote_entry(&mut self, index: u64, entry: &LogEntry<E>) -> Result<(), Self::Error>;
}
