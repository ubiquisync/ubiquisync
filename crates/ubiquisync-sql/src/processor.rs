use ubiquisync_core::{
    hlc::{HlcError, HlcService, wall_ms},
    log_entry::LogEntry,
    uuid::Uuid,
};

use crate::{
    db::{Db, DbError},
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
    tracker::{LogTracker, LogTrackerError},
};

// The processor is reachable today only from the in-crate `test_support`
// harness; the public ingestion entry point that drives it is not wired up yet.
// Suppress the resulting dead-code lints rather than prematurely commit to a
// `pub` surface — these clear once a caller lands.
#[allow(dead_code)]
pub struct Processor<R: Reducer, D: Db, T> {
    reducer: R,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
    tracker: T,
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> Processor<R, D, T> {
    /// Wire up a processor against `db`: open the HLC register and the tracker's
    /// op-log (both namespaced by `prefix`), seed the in-memory clock from the
    /// persisted state, and take ownership of `reducer`. The two schema setups
    /// run as their own autocommit DDL — they are additive and safe to commit
    /// before any entry is ingested.
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

    /// The backend this processor writes through. For test/diagnostic reads of
    /// the op-log and clock register the processor manages.
    pub(crate) fn db(&self) -> &D {
        &self.db
    }

    /// Ingest one log entry atomically: observe its HLC timestamp, record it in
    /// the op-log, and apply the reducer's writes in one all-or-nothing batch.
    ///
    /// Re-ingesting an applied `(client_id, client_idx)` hits the op-log PK and
    /// surfaces as [`DbError::UniqueViolation`](crate::db::DbError::UniqueViolation)
    /// (the batch rolls back); it is not swallowed. Callers dedup against their
    /// known last-ingested index — we can't portably tell an op-log conflict from
    /// a reducer's own constraint, so reducers must use idempotent upserts.
    pub(crate) async fn process_one(
        &mut self,
        client_id: &Uuid,
        client_idx: u64,
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
            .track_one(client_id, client_idx, entry, batch.as_mut())?;
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
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessorError<E> {
    // No `#[from]` here: a blanket `From<E>` would overlap the concrete
    // `From<DbError>` / `From<CodecError>` impls, so reducer errors are mapped
    // explicitly at the call sites.
    #[error("reducer error: {0}")]
    Reducer(E),
    #[error("hlc error: {0}")]
    Hlc(#[from] HlcError<DbError>),
    #[error("tracker error: {0}")]
    Tracker(#[from] LogTrackerError),
    #[error("db error: {0}")]
    Db(#[from] DbError),
}
