//! The storage seam: traits a backend implements to expose a peer's log
//! stream to the sync engine — the read side ([`LogSource`]) and the write
//! side ([`LogEntrySink`]). Both are generic over the op vocabulary `E` so one
//! storage layer can carry any data domain. Concrete backends (e.g. an
//! on-disk segment store) live in companion crates.

use std::ops::ControlFlow;

use crate::codec::DecodedEntry;
use crate::hlc::Timestamp;
use crate::uuid::Uuid;

use super::error::SyncError;

/// Encoding strategy for log entries. Implementations encode and append
/// entries to underlying storage, choosing the wire format (device vs server).
///
/// Returns the peer's cursor position after the write — i.e. the number of
/// entries now in the stream, which is the index of the next entry to write.
pub trait LogEntrySink<E> {
    fn write(
        &mut self,
        ts: Timestamp,
        user_id: Option<Uuid>,
        entries: &[E],
    ) -> Result<u64, SyncError>;
}

/// Read-side of log storage: discover peers and read their entries.
///
/// Generic over the entry type `E` — the source handles decoding from whatever
/// underlying format into [`DecodedEntry`] values (which may include expunged
/// markers).
pub trait LogSource<E> {
    /// Returns the IDs of all peers whose log files are locally available.
    fn list_peers(&self) -> Vec<Uuid>;

    /// Reads entries for `peer`, starting at stream index `start_entry_idx`,
    /// feeding each `(index, entry)` pair to `consumer`. The consumer returns
    /// [`ControlFlow::Break`] to stop early (carrying a result), or
    /// [`ControlFlow::Continue`] to keep reading.
    fn read_entries<F, Err>(
        &self,
        peer: &Uuid,
        start_entry_idx: u64,
        consumer: F,
    ) -> Result<(), Err>
    where
        Err: From<SyncError>,
        F: FnMut(u64, DecodedEntry<E>) -> ControlFlow<Result<(), Err>>;
}
