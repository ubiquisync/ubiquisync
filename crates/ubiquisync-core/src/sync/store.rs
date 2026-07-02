//! Storage traits a backend implements to expose a peer's log stream to the
//! sync engine — the read side ([`LogSource`]) and the write side
//! ([`LogEntrySink`]). Both are generic over the op vocabulary `E` so one
//! storage layer can carry any data domain. Concrete backends (e.g. an
//! on-disk segment store) live in companion crates.

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
    /// Encode and append `entries` under timestamp `ts` (and `server_user_id`
    /// in server mode), returning the stream's new cursor position — the entry
    /// count after the write.
    fn write(
        &mut self,
        ts: Timestamp,
        server_user_id: Option<Uuid>,
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

    /// Reads the next batch of `peer`'s entries at or after stream index
    /// `start_entry_idx`, ascending (expunged markers included), returning each
    /// as an `(index, entry)` pair. An **empty** batch means the stream is
    /// drained at `start_entry_idx` — there is nothing more to read.
    ///
    /// The source, not the caller, sizes the batch: it returns a natural unit it
    /// already has in hand — typically one decoded segment — so chunking falls
    /// to the side that knows the real boundaries and can bound each read by a
    /// quantity it controls (segment size), rather than an entry count the
    /// caller guesses. The driver keeps calling with an advancing cursor until a
    /// call comes back empty, so returning fewer entries than remain is fine;
    /// only an empty result signals the end.
    ///
    /// Synchronous on purpose: a source decodes from local segment files, so the
    /// read has no `.await`, while the apply side that consumes the batch is
    /// async.
    fn read_entries(
        &self,
        peer: &Uuid,
        start_entry_idx: u64,
    ) -> Result<Vec<(u64, DecodedEntry<E>)>, SyncError>;
}
