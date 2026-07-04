//! The ingestion driver: applies one log entry atomically, and exposes the
//! stored log back as a [`Replica`](ubiquisync_core::sync::Replica).
//!
//! A `Processor` pairs a [`Reducer`](crate::reducer) with an
//! [`HlcService`](ubiquisync_core::hlc) clock and a
//! [`LogTracker`](crate::tracker): ingesting an entry advances the clock, records
//! it via the tracker, and applies the reducer's writes in one all-or-nothing
//! batch. It always implements [`HasCursors`] and [`LogProcessor`]; when its
//! tracker keeps full history ([`HistoryTracker`]) it also implements
//! [`LogSource`], and so is a `Replica`.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::lock::Mutex as AsyncMutex;
use ubiquisync_core::{
    codec::DecodedEntry,
    event::{EventBus, EventHandler, Publisher, RoutableEvent, Subscription},
    hlc::{HlcError, HlcService, Timestamp, wall_ms},
    log_entry::LogEntry,
    sync::{
        Applied, CursorStream, CursorsEvent, HasCursors, LogProcessor, LogSource, PeerCursors,
        SyncError,
    },
    uuid::Uuid,
};

use crate::{
    db::{Db, DbError, DbRow, DbValue},
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
    store::SqlStore,
    tracker::{HistoryTracker, LogTracker, LogTrackerError},
};

// Crate-private until the public store that drives it lands. `#[allow(dead_code)]`
// because the only caller today is the in-crate `test_support` harness, so a
// plain build sees it as unused.
#[allow(dead_code)]
pub(crate) struct Processor<R: Reducer, D: Db, T, E: EventHandler<R::Event>> {
    self_id: Uuid,
    // Behind an async mutex: it hands out `&mut` for the reducer's `prepare`, and
    // holding it across an apply serializes writes (single-writer log store).
    reducer: AsyncMutex<R>,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
    tracker: T,
    // In-memory version vector, seeded at open and advanced on each apply. Backs
    // the idempotent-drop fast path and the `watch_cursors` broadcast.
    cursors: Mutex<PeerCursors>,
    watchers: Mutex<Vec<mpsc::UnboundedSender<CursorsEvent>>>,
    event_publish: E::Publish,
    event_handler: E,
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>, E: EventHandler<R::Event>> Processor<R, D, T, E> {
    /// Open a processor against `db`: set up HLC storage and the tracker (both
    /// namespaced by `prefix`), seed the clock and cursor view from persisted
    /// state, and take ownership of `reducer`.
    pub async fn open(
        reducer: R,
        db: D,
        prefix: &str,
        self_id: Uuid,
    ) -> Result<Self, ProcessorError<R::Error>> {
        let hlc = HlcService::open(SqlHlcStorage::open(&db, prefix).await?)?;
        let tracker = T::init(&db, prefix).await?;
        let cursors = tracker.all_cursors(&db).await?;
        let (event_publish, event_handler) = E::init();
        Ok(Self {
            reducer: AsyncMutex::new(reducer),
            self_id,
            db,
            hlc,
            tracker,
            event_publish,
            event_handler,
            cursors: Mutex::new(cursors),
            watchers: Mutex::new(Vec::new()),
        })
    }

    /// Apply a local write: mint a fresh entry under `self_id` and ingest it,
    /// advancing self's cursor.
    pub async fn exec(
        &self,
        server_user_id: Option<Uuid>,
        op: R::Op,
    ) -> Result<(), ProcessorError<R::Error>> {
        let mut reducer = self.reducer.lock().await;
        let entry_idx = self.cached_cursor(&self.self_id);
        let events = self
            .ingest_entry_or_local(
                &mut reducer,
                &self.self_id,
                entry_idx,
                None,
                server_user_id,
                &op,
            )
            .await?;
        // Emit outside the reducer lock (see `ingest_entry_or_local`).
        drop(reducer);
        for event in events {
            self.event_publish.publish(event);
        }
        Ok(())
    }

    /// The handler this processor emits into — subscribe off it.
    pub fn event_handler(&self) -> &E {
        &self.event_handler
    }

    /// The backend this processor writes through — for tests and diagnostics.
    pub(crate) fn db(&self) -> &D {
        &self.db
    }

    /// Ingest one entry into the open write section — caller holds the reducer
    /// lock. Advances the HLC, records via the tracker, and applies the reducer's
    /// writes in one all-or-nothing batch. No dedup: a repeated
    /// `(peer_id, entry_idx)` fails the tracker's unique key and rolls back.
    async fn ingest_entry(
        &self,
        reducer: &mut R,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<Vec<R::Event>, ProcessorError<R::Error>> {
        self.ingest_entry_or_local(
            reducer,
            peer_id,
            entry_idx,
            Some(entry.timestamp),
            entry.server_user_id,
            &entry.op,
        )
        .await
    }

    async fn ingest_entry_or_local(
        &self,
        reducer: &mut R,
        peer_id: &Uuid,
        entry_idx: u64,
        timestamp: Option<Timestamp>,
        server_user_id: Option<Uuid>,
        op: &R::Op,
    ) -> Result<Vec<R::Event>, ProcessorError<R::Error>> {
        let prepare_state = reducer
            .prepare(&self.db, op)
            .await
            .map_err(ProcessorError::Reducer)?;
        let mut batch = self.db.new_batch();
        let timestamp = if let Some(timestamp) = timestamp {
            self.hlc.observe(timestamp, wall_ms(), batch.as_mut())?;
            timestamp
        } else {
            self.hlc.now(batch.as_mut())?
        };
        self.tracker.track_one(
            peer_id,
            entry_idx,
            timestamp,
            server_user_id,
            op,
            batch.as_mut(),
        )?;
        let apply_state = reducer
            .apply(batch.as_mut(), timestamp, op, prepare_state)
            .map_err(ProcessorError::Reducer)?;
        let batch_result = batch.commit().await?;
        let event = reducer
            .post_apply(apply_state, &batch_result)
            .map_err(ProcessorError::Reducer)?;
        // Advance under the reducer lock so the cursor reflects this commit before
        // the next apply checks it. The caller emits `event` *after* releasing the
        // lock — user-provided `publish` must not run in the write critical section.
        self.advance_cursor(peer_id, entry_idx + 1);
        Ok(event)
    }
    /// Record the expunged marker at `(peer_id, entry_idx)` — caller holds the
    /// reducer lock. No clock tick and no reducer work: just the tracker row that
    /// occupies the stream index.
    async fn ingest_expunged(
        &self,
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

    /// Raw ingest at a caller-given index — no dedup gate
    /// ([`apply`](LogProcessor::apply)'s job). Advances the cursor like any
    /// ingest. Test-only; local writes go through `exec`.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn process_one(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<(), ProcessorError<R::Error>> {
        let mut reducer = self.reducer.lock().await;
        let events = self
            .ingest_entry(&mut reducer, peer_id, entry_idx, entry)
            .await?;
        drop(reducer);
        for event in events {
            self.event_publish.publish(event);
        }
        Ok(())
    }
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T, E: EventHandler<R::Event>> Processor<R, D, T, E> {
    fn cached_cursor(&self, peer: &Uuid) -> u64 {
        lock(&self.cursors).get(peer).copied().unwrap_or(0)
    }

    /// Raise `peer`'s cached cursor and broadcast the advance to watchers.
    fn advance_cursor(&self, peer: &Uuid, next: u64) {
        let advanced = {
            let mut cursors = lock(&self.cursors);
            let slot = cursors.entry(*peer).or_insert(0);
            if next > *slot {
                *slot = next;
                true
            } else {
                false
            }
        };
        if advanced {
            let mut delta = PeerCursors::new();
            delta.insert(*peer, next);
            lock(&self.watchers).retain(|tx| {
                tx.unbounded_send(CursorsEvent::Advanced(delta.clone()))
                    .is_ok()
            });
        }
    }
}

/// Lock a sync mutex, recovering the guard if a prior holder panicked: the
/// protected cursor/watcher state can't be left corrupt by a panic, so this
/// beats propagating a poison to every subsystem sharing the processor.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[async_trait]
impl<R: Reducer, D: Db, T: Send + Sync, E: EventHandler<R::Event> + Send + Sync> HasCursors
    for Processor<R, D, T, E>
where
    E::Publish: Send + Sync,
{
    async fn cursors(&self) -> Result<PeerCursors, SyncError> {
        Ok(lock(&self.cursors).clone())
    }

    fn watch_cursors(&self) -> CursorStream {
        let (tx, rx) = mpsc::unbounded();
        // Hold both locks across snapshot + registration (cursors before
        // watchers, the order advance_cursor also acquires them) so a concurrent
        // advance can't slip its mutation and broadcast between the two and leave
        // this subscriber stale. Neither section awaits.
        let cursors = lock(&self.cursors);
        let mut watchers = lock(&self.watchers);
        let _ = tx.unbounded_send(CursorsEvent::Snapshot(cursors.clone()));
        watchers.retain(|w| !w.is_closed()); // drop subscribers that went away
        watchers.push(tx);
        Box::pin(rx)
    }
}

#[async_trait]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>, E: EventHandler<R::Event> + Send + Sync>
    LogProcessor<R::Op> for Processor<R, D, T, E>
where
    R::Error: std::error::Error + Send + Sync + 'static,
    E::Publish: Send + Sync,
{
    async fn apply(
        &self,
        peer: Uuid,
        index: u64,
        entry: DecodedEntry<R::Op>,
    ) -> Result<Applied, SyncError> {
        // Fast path: drop an already-applied re-delivery without taking the lock.
        if index < self.cached_cursor(&peer) {
            return Ok(Applied { new: false });
        }
        let mut reducer = self.reducer.lock().await;
        // Re-check under the lock and enforce contiguity: a concurrent apply may
        // have advanced past `index`. `< cursor` is a re-delivery (drop);
        // `> cursor` is a gap the caller must not create — the scalar cursor
        // can't hold a hole, so reject rather than jump past the missing entries.
        // `== cursor` is the only-once gate, so the reducer need not be idempotent.
        let cursor = self.cached_cursor(&peer);
        if index < cursor {
            return Ok(Applied { new: false });
        }
        if index > cursor {
            return Err(SyncError::CursorMismatch {
                expected_idx: cursor,
                actual_idx: index,
            });
        }
        // `ingest_entry_or_local` advances the cursor after its commit; the
        // expunged path does no reducer work, so it advances here instead.
        let outcome: Result<Vec<R::Event>, ProcessorError<R::Error>> = match entry {
            DecodedEntry::LogEntry(e) => self.ingest_entry(&mut reducer, &peer, index, &e).await,
            DecodedEntry::Expunged(hash) => {
                let outcome = self.ingest_expunged(&peer, index, &hash).await;
                if outcome.is_ok() {
                    self.advance_cursor(&peer, index + 1);
                }
                outcome.map(|()| Vec::new())
            }
        };
        // Emit outside the reducer lock (see `ingest_entry_or_local`).
        drop(reducer);
        match outcome {
            Ok(events) => {
                for event in events {
                    self.event_publish.publish(event);
                }
                Ok(Applied { new: true })
            }
            // Backstop if the gate is bypassed (a direct process_one, or an
            // unseeded cache): the batch rolled back, so it's a no-op.
            Err(ProcessorError::Db(DbError::UniqueViolation)) => Ok(Applied { new: false }),
            Err(e) => Err(SyncError::Backend(Box::new(e))),
        }
    }
}

#[async_trait]
impl<R: Reducer, D: Db, T: HistoryTracker<R::Op>, E: EventHandler<R::Event> + Send + Sync>
    LogSource<R::Op> for Processor<R, D, T, E>
where
    R::Error: std::error::Error + Send + Sync + 'static,
    E::Publish: Send + Sync,
{
    async fn read_since(
        &self,
        peer: Uuid,
        from: u64,
    ) -> Result<Vec<(u64, DecodedEntry<R::Op>)>, SyncError> {
        // One oplog page per call; the caller loops with an advancing `from`.
        const PAGE: u64 = 256;
        self.tracker
            .read_entries(&self.db, &peer, from, PAGE)
            .await
            .map_err(|e| SyncError::Backend(Box::new(e)))
    }
}

// The engine implements `Store` over its *own* op/event types — no projection.
// A domain wrapper (e.g. a `-tables` store) layers `Into`/`TryFrom` on top to
// expose an app slice; that stays out of here.
#[async_trait]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>>
    ubiquisync_core::store::Store<R::Op, ProcessorError<BoxError>, R::Event>
    for Processor<R, D, T, EventBus<R::Event>>
where
    R::Event: RoutableEvent,
    R::Error: std::error::Error + Send + Sync + 'static,
    <R::Event as RoutableEvent>::Target: Send + Sync,
{
    async fn exec(
        &self,
        server_user_id: Option<Uuid>,
        op: R::Op,
    ) -> Result<(), ProcessorError<BoxError>> {
        // Inherent `Processor::exec` (shadows this trait method), then erase the
        // reducer error behind `BoxError` so the Store surface is one uniform type
        // across reducers while keeping the `ProcessorError` variants matchable.
        Processor::exec(self, server_user_id, op)
            .await
            .map_err(|e| match e {
                ProcessorError::Reducer(r) => ProcessorError::Reducer(Box::new(r) as BoxError),
                ProcessorError::Hlc(x) => ProcessorError::Hlc(x),
                ProcessorError::Tracker(x) => ProcessorError::Tracker(x),
                ProcessorError::Db(x) => ProcessorError::Db(x),
                ProcessorError::Sync(x) => ProcessorError::Sync(x),
            })
    }

    fn watch(&self, target: <R::Event as RoutableEvent>::Target) -> Subscription<R::Event> {
        self.event_handler().subscribe(target)
    }
}

#[async_trait]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> SqlStore<R::Op, R::Event>
    for Processor<R, D, T, EventBus<R::Event>>
where
    R::Event: RoutableEvent,
    R::Error: std::error::Error + Send + Sync + 'static,
    <R::Event as RoutableEvent>::Target: Send + Sync,
{
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        self.db().query(sql, params).await
    }

    fn dialect(&self) -> crate::dialect::SqlDialect {
        self.db().dialect()
    }
}

/// A reducer error erased to a trait object — the uniform reducer-error type the
/// [`Store`](ubiquisync_core::store::Store) surface exposes across reducers.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

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
    /// A [`SyncError`] surfaced from the sync layer.
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
}
