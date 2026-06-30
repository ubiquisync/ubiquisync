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

pub struct Processor<R: Reducer, D: Db, T> {
    reducer: R,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
    tracker: T,
}

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

    /// Ingest one log entry. Returns `Ok(None)` when the entry was already
    /// ingested — a duplicate `(client_id, client_idx)` fails the batch's unique
    /// constraint, rolling everything back, so nothing is re-applied.
    pub(crate) async fn process_one(
        &mut self,
        client_id: &Uuid,
        client_idx: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<Option<R::Event>, ProcessorError<R::Error>> {
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
        let batch_result = match batch.commit().await {
            Ok(result) => result,
            Err(DbError::UniqueViolation) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let event = self
            .reducer
            .post_apply(apply_state, &batch_result)
            .map_err(ProcessorError::Reducer)?;
        Ok(Some(event))
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
