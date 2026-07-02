//! [`LogProcessor`]: the write side of a replica.

use async_trait::async_trait;

use crate::codec::DecodedEntry;
use crate::sync::cursors::HasCursors;
use crate::uuid::Uuid;

use super::error::SyncError;

/// The apply target of replication: absorbs entries addressed by `(peer, index)`.
///
/// Multi-writer — it accepts any origin, so it can relay and merge. (A file log,
/// which writes only its own origin, is [`FileLogSink`](super::FileLogSink)
/// instead.)
///
/// `apply` is idempotent per `(peer, index)`: multi-channel delivery means the
/// same entry can arrive twice, so an already-seen index is a no-op reported as
/// [`Applied::new`] `== false`.
///
/// `&self` and `?Send`: one processor is shared (`Rc<dyn LogProcessor>`) as the
/// apply target of several sources while also read as a
/// [`LogSource`](super::LogSource), so it mutates through interior mutability.
/// `?Send` matches the `?Send` `Db` backend (wasm/Workers).
#[async_trait(?Send)]
pub trait LogProcessor<E>: HasCursors {
    /// Apply one entry at `(peer, index)`, advancing the cursor for `peer` to
    /// `index + 1`. Idempotent (see the trait docs).
    async fn apply(
        &self,
        peer: Uuid,
        index: u64,
        entry: DecodedEntry<E>,
    ) -> Result<Applied, SyncError>;
}

/// Outcome of [`LogProcessor::apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// `true` if newly applied; `false` if a re-delivery that changed nothing.
    pub new: bool,
}
