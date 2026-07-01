//! The ingestion driver that applies one log entry atomically.
//!
//! A `Processor` pairs a [`Reducer`](crate::reducer) with an
//! [`HlcService`](ubiquisync_core::hlc) clock and a
//! [`LogTracker`](crate::tracker), so that ingesting an entry advances the
//! clock, hands the entry to the tracker, and applies the reducer's writes in a
//! single all-or-nothing batch. [`ProcessorError`] tags a failure by which of
//! those collaborators produced it.

use async_trait::async_trait;
use ubiquisync_core::{
    hlc::{HlcError, HlcService, wall_ms},
    log_entry::LogEntry,
    sync::{LogProcessor, SyncError},
    uuid::Uuid,
};

use crate::{
    db::{Db, DbError},
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
    tracker::{LogTracker, LogTrackerError},
};

// Crate-private until the public ingestion entry point that drives it is wired
// up: exposing the type while its constructor stayed `pub(crate)` would let
// downstream see a `Processor` it can't build. `#[allow(dead_code)]` because the
// only caller today is the in-crate `test_support` harness, so a normal build
// sees it as unused — clears once a caller lands.
#[allow(dead_code)]
pub(crate) struct Processor<R: Reducer, D: Db, T> {
    reducer: R,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
    tracker: T,
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> Processor<R, D, T> {
    /// Open a processor against `db`: set up the HLC storage and initialize the
    /// tracker (both namespaced by `prefix`), seed the in-memory clock from
    /// persisted state, and take ownership of `reducer`. Any setup they perform
    /// runs before the first entry is ingested.
    pub(crate) async fn open(
        reducer: R,
        db: D,
        prefix: &str,
    ) -> Result<Self, ProcessorError<R::Error>> {
        let hlc = HlcService::open(SqlHlcStorage::open(&db, prefix).await?)?;
        let tracker = T::init(&db, prefix).await?;
        Ok(Self {
            reducer,
            db,
            hlc,
            tracker,
        })
    }

    /// The backend this processor writes through — for tests and diagnostics
    /// that read back what it has persisted.
    pub(crate) fn db(&self) -> &D {
        &self.db
    }

    /// Ingest one log entry atomically: advance the HLC with the entry's
    /// timestamp, hand the entry to the tracker, and apply the reducer's writes —
    /// all in one all-or-nothing batch that rolls back if any step fails.
    ///
    /// No deduplication happens here. Re-ingesting an already-applied
    /// `(peer_id, entry_idx)` re-runs the tracker and reducer against it;
    /// whether that errors or is absorbed is up to those collaborators. Callers
    /// should dedup against their last-ingested index, and reducers should apply
    /// idempotently UNLESS the tracker ensures only-once consistency.
    pub(crate) async fn process_one(
        &mut self,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<R::Event, ProcessorError<R::Error>> {
        let op = &entry.op;
        let timestamp = entry.timestamp;
        let prepare_state = self
            .reducer
            .prepare(&self.db, op)
            .await
            .map_err(ProcessorError::Reducer)?;
        let mut batch = self.db.new_batch();
        self.hlc.observe(timestamp, wall_ms(), batch.as_mut())?;
        self.tracker
            .track_one(peer_id, entry_idx, entry, batch.as_mut())?;
        let apply_state = self
            .reducer
            .apply(batch.as_mut(), timestamp, op, prepare_state)
            .map_err(ProcessorError::Reducer)?;
        let batch_result = batch.commit().await?;
        let event = self
            .reducer
            .post_apply(apply_state, &batch_result)
            .map_err(ProcessorError::Reducer)?;
        Ok(event)
    }

    /// Record the expunged marker at `(peer_id, entry_idx)` atomically: hand the
    /// `hash` to the tracker and commit just that row. Unlike an applied entry
    /// there is no reducer work and no clock to advance — the marker carries no
    /// timestamp — so this only occupies the stream index, letting the peer's
    /// cursor advance past the expunged gap.
    pub(crate) async fn process_expunged(
        &mut self,
        peer_id: &Uuid,
        entry_idx: u64,
        hash: &blake3::Hash,
    ) -> Result<(), ProcessorError<R::Error>> {
        let mut batch = self.db.new_batch();
        self.tracker
            .track_expunged(peer_id, entry_idx, hash, batch.as_mut())?;
        batch.commit().await?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> LogProcessor<R::Op> for Processor<R, D, T> {
    type Error = ProcessorError<R::Error>;

    async fn get_peer_cursor(&self, peer_id: &Uuid) -> Result<u64, Self::Error> {
        Ok(self.tracker.peer_cursor(&self.db, peer_id).await?)
    }

    /// Applies the entry and advances the peer's cursor atomically: the cursor
    /// is derived from what the tracker has recorded, so the same commit that
    /// records the entry advances it. The per-entry
    /// [`R::Event`](Reducer::Event) is dropped here; the sync engine has no
    /// channel for it yet.
    async fn apply_entry(
        &mut self,
        peer_id: &Uuid,
        index: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<(), Self::Error> {
        self.process_one(peer_id, index, entry).await?;
        Ok(())
    }

    async fn save_expunged_entry(
        &mut self,
        peer_id: &Uuid,
        index: u64,
        hash: &blake3::Hash,
    ) -> Result<(), Self::Error> {
        self.process_expunged(peer_id, index, hash).await
    }
}

/// A failure while ingesting an entry, tagged by the stage that produced it.
#[derive(Debug, thiserror::Error)]
pub enum ProcessorError<E> {
    /// The reducer failed; `E` is its own error type.
    // No `#[from]`: a blanket `From<E>` would clash with `From<DbError>` when a
    // reducer sets `Error = DbError`. Mapped explicitly at the call sites.
    #[error("reducer error: {0}")]
    Reducer(E),
    /// Advancing or persisting the HLC failed.
    #[error("hlc error: {0}")]
    Hlc(#[from] HlcError<DbError>),
    /// The tracker failed to record the entry.
    #[error("tracker error: {0}")]
    Tracker(#[from] LogTrackerError),
    /// A backend operation failed — e.g. the batch commit was rejected.
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// A [`SyncError`] from the sync engine — in practice a failed source read,
    /// which [`PullSynchronizer`](ubiquisync_core::sync::PullSynchronizer) maps
    /// into this processor's error type as it drives the sync. Required by
    /// [`ubiquisync_core::sync::LogProcessor::Error`]'s `From<SyncError>` bound.
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
}
