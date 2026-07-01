//! [`LogProcessor`]: what the sync engine applies a peer's entries into. A
//! processor — typically a materialized store — absorbs entries and remembers
//! how far it has read each peer's stream.

use async_trait::async_trait;

use crate::log_entry::LogEntry;
use crate::uuid::Uuid;

use super::error::SyncError;

/// A store the sync engine applies a peer's entries into, tracking how far it
/// has read each peer's stream.
///
/// Both [`apply_entry`](LogProcessor::apply_entry) and
/// [`save_expunged_entry`](LogProcessor::save_expunged_entry) advance the peer's
/// cursor to `index + 1` atomically with the record they write, so a partial
/// write commits nothing and the next pass re-reads from the same index.
///
/// # Re-delivery
///
/// The engine can re-deliver an already-applied `(peer_id, index)`. An
/// implementation must keep that from applying twice, in one of two ways:
///
/// - reject a repeated `(peer_id, index)` so the apply rolls back (e.g. a
///   unique key on it), or
/// - make the applied effect idempotent, so re-applying it is a no-op.
///
/// Which one holds is a property of the implementation, and a reducer's
/// idempotency requirement follows from it: behind a rejecting store a reducer
/// need not be idempotent; behind one that does not reject, it must be.
#[async_trait(?Send)]
pub trait LogProcessor<E> {
    /// The processor's own error type; must absorb [`SyncError`] so the engine
    /// can surface transport failures through it.
    type Error: From<SyncError>;

    /// Returns the next-entry index for the given peer; `0` if the peer has
    /// never been seen.
    async fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error>;

    /// Applies one entry, identified by its stream `index` within `peer_id`'s
    /// stream, and advances the peer's cursor to `index + 1` atomically. Must
    /// honor the idempotency/atomicity contract described on the trait.
    async fn apply_entry(
        &mut self,
        peer_id: &Uuid,
        index: u64,
        entry: &LogEntry<E>,
    ) -> Result<(), Self::Error>;

    /// Records the expunged marker at stream `index` within `peer_id`'s stream,
    /// advancing the peer's cursor past it without materializing anything. `hash`
    /// names the entry that was expunged. Like [`apply_entry`](Self::apply_entry),
    /// the cursor advance is atomic with the record.
    async fn save_expunged_entry(
        &mut self,
        peer_id: &Uuid,
        index: u64,
        hash: &blake3::Hash,
    ) -> Result<(), Self::Error>;
}
