//! Per-origin replication cursors — the version vector replicas exchange.

use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;

use crate::uuid::Uuid;

use super::error::SyncError;

/// Per-origin position: `peer id → next entry index`. The value for a peer is
/// what to pass [`read_since`](super::LogSource::read_since) for the next entry;
/// an absent key means `0`.
///
/// A version vector: merge by pointwise `max`, diff to get the gap to pull.
pub type PeerCursors = HashMap<Uuid, u64>;

/// An event from [`watch_cursors`](HasCursors::watch_cursors).
///
/// First event is a full `Snapshot`, later events `Advanced` deltas of the
/// origins that moved; fold each in by pointwise `max`. Carrying cursor state
/// (not entries) makes it loss-tolerant: a dropped or coalesced event costs
/// nothing — the next one still names the current position, and the gap is
/// refetched via `read_since`.
#[derive(Debug, Clone)]
pub enum CursorsEvent {
    /// Full cursor vector at subscription time.
    Snapshot(PeerCursors),
    /// Origins that advanced since the last event, mapped to their new index.
    Advanced(PeerCursors),
}

/// Stream from [`watch_cursors`](HasCursors::watch_cursors). Boxed and `?Send`
/// to keep [`HasCursors`] object-safe; ending means the watch closed.
pub type CursorStream = Pin<Box<dyn Stream<Item = CursorsEvent>>>;

/// Snapshot or watch a replica's cursor vector — the shared base of
/// [`LogProcessor`](super::LogProcessor) and [`LogSource`](super::LogSource).
#[async_trait(?Send)]
pub trait HasCursors {
    /// Snapshot of the current cursor vector.
    async fn cursors(&self) -> Result<PeerCursors, SyncError>;

    /// The cursor for a single `peer` — its next entry index, `0` if unseen.
    /// Defaults to reading the whole vector; override where one peer is cheaper.
    async fn get_cursor(&self, peer: Uuid) -> Result<u64, SyncError> {
        Ok(self.cursors().await?.get(&peer).copied().unwrap_or(0))
    }

    /// Live cursor progress: a first [`Snapshot`](CursorsEvent::Snapshot), then
    /// [`Advanced`](CursorsEvent::Advanced) deltas.
    fn watch_cursors(&self) -> CursorStream;
}
