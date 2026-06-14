//! The apply seam: [`LogProcessor`] is what the sync engine drives. A
//! processor — typically a materialized store — absorbs remote entries and
//! remembers how far it has read each peer's stream.

use crate::log_entry::LogEntry;
use crate::uuid::Uuid;

use super::error::LogStoreError;

/// A store that can absorb remote entries and remember how far it has read each
/// peer's stream.
pub trait LogProcessor<E> {
    type Error: From<LogStoreError>;

    /// Returns the next-entry index for the given peer; `0` if the peer has
    /// never been seen.
    fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error>;

    /// Persists the next-entry index for the given peer.
    fn save_peer_cursor(&mut self, peer_id: &Uuid, next_entry_idx: u64) -> Result<(), Self::Error>;

    /// Applies one remote entry, identified by its stream index.
    fn apply_remote_entry(&mut self, index: u64, entry: &LogEntry<E>) -> Result<(), Self::Error>;
}
