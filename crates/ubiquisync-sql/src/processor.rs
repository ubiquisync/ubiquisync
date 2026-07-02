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

use std::cell::RefCell;

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::lock::Mutex;
use ubiquisync_core::{
    codec::DecodedEntry,
    hlc::{HlcError, HlcService, Timestamp, wall_ms},
    log_entry::LogEntry,
    sync::{
        Applied, CursorStream, CursorsEvent, HasCursors, LogProcessor, LogSource, PeerCursors,
        SyncError,
    },
    uuid::Uuid,
};

use crate::{
    db::{Db, DbError},
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
    tracker::{HistoryTracker, LogTracker, LogTrackerError},
};

// Crate-private until the public store that drives it lands. `#[allow(dead_code)]`
// because the only caller today is the in-crate `test_support` harness, so a
// plain build sees it as unused.
#[allow(dead_code)]
pub(crate) struct Processor<R: Reducer, D: Db, T> {
    self_id: Uuid,
    // Behind an async mutex: it hands out `&mut` for the reducer's `prepare`, and
    // holding it across an apply serializes writes (single-writer log store).
    reducer: Mutex<R>,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
    tracker: T,
    // In-memory version vector, seeded at open and advanced on each apply. Backs
    // the idempotent-drop fast path and the `watch_cursors` broadcast.
    cursors: RefCell<PeerCursors>,
    watchers: RefCell<Vec<mpsc::UnboundedSender<CursorsEvent>>>,
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> Processor<R, D, T> {
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
        Ok(Self {
            reducer: Mutex::new(reducer),
            self_id,
            db,
            hlc,
            tracker,
            cursors: RefCell::new(cursors),
            watchers: RefCell::new(Vec::new()),
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
        self.ingest_entry_or_local(
            &mut reducer,
            &self.self_id,
            entry_idx,
            None,
            server_user_id,
            &op,
        )
        .await?;
        self.advance_cursor(&self.self_id, entry_idx + 1);
        Ok(())
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
    ) -> Result<R::Event, ProcessorError<R::Error>> {
        let res = self
            .ingest_entry_or_local(
                reducer,
                peer_id,
                entry_idx,
                Some(entry.timestamp),
                entry.server_user_id,
                &entry.op,
            )
            .await?;
        Ok(res)
    }

    async fn ingest_entry_or_local(
        &self,
        reducer: &mut R,
        peer_id: &Uuid,
        entry_idx: u64,
        timestamp: Option<Timestamp>,
        server_user_id: Option<Uuid>,
        op: &R::Op,
    ) -> Result<R::Event, ProcessorError<R::Error>> {
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
        reducer
            .post_apply(apply_state, &batch_result)
            .map_err(ProcessorError::Reducer)
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

    /// Raw ingest that returns the reducer's event — no dedup gate (that is
    /// [`apply`](LogProcessor::apply)'s job). Test-only for now; the local-write
    /// path (`tx`) will wrap `ingest_entry` with cursor-advance and a broadcast.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) async fn process_one(
        &self,
        peer_id: &Uuid,
        entry_idx: u64,
        entry: &LogEntry<R::Op>,
    ) -> Result<R::Event, ProcessorError<R::Error>> {
        let mut reducer = self.reducer.lock().await;
        self.ingest_entry(&mut reducer, peer_id, entry_idx, entry)
            .await
    }
}

#[allow(dead_code)]
impl<R: Reducer, D: Db, T> Processor<R, D, T> {
    fn cached_cursor(&self, peer: &Uuid) -> u64 {
        self.cursors.borrow().get(peer).copied().unwrap_or(0)
    }

    /// Raise `peer`'s cached cursor and broadcast the advance to watchers.
    fn advance_cursor(&self, peer: &Uuid, next: u64) {
        let advanced = {
            let mut cursors = self.cursors.borrow_mut();
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
            self.watchers.borrow_mut().retain(|tx| {
                tx.unbounded_send(CursorsEvent::Advanced(delta.clone()))
                    .is_ok()
            });
        }
    }
}

#[async_trait(?Send)]
impl<R: Reducer, D: Db, T> HasCursors for Processor<R, D, T> {
    async fn cursors(&self) -> Result<PeerCursors, SyncError> {
        Ok(self.cursors.borrow().clone())
    }

    fn watch_cursors(&self) -> CursorStream {
        // Synchronous: registration and the snapshot happen without an await, so
        // no apply can advance the cursor in between and be missed.
        let (tx, rx) = mpsc::unbounded();
        let _ = tx.unbounded_send(CursorsEvent::Snapshot(self.cursors.borrow().clone()));
        let mut watchers = self.watchers.borrow_mut();
        watchers.retain(|w| !w.is_closed()); // drop subscribers that went away
        watchers.push(tx);
        Box::pin(rx)
    }
}

#[async_trait(?Send)]
impl<R: Reducer, D: Db, T: LogTracker<R::Op>> LogProcessor<R::Op> for Processor<R, D, T>
where
    R::Error: std::error::Error + 'static,
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
        let outcome = match entry {
            DecodedEntry::LogEntry(e) => self
                .ingest_entry(&mut reducer, &peer, index, &e)
                .await
                .map(|_| ()),
            DecodedEntry::Expunged(hash) => self.ingest_expunged(&peer, index, &hash).await,
        };
        match outcome {
            // Advance under the lock, so the cursor reflects this commit before
            // the next apply can check it.
            Ok(()) => {
                self.advance_cursor(&peer, index + 1);
                Ok(Applied { new: true })
            }
            // Backstop if the gate is bypassed (a direct process_one, or an
            // unseeded cache): the batch rolled back, so it's a no-op.
            Err(ProcessorError::Db(DbError::UniqueViolation)) => Ok(Applied { new: false }),
            Err(e) => Err(SyncError::Backend(Box::new(e))),
        }
    }
}

#[async_trait(?Send)]
impl<R: Reducer, D: Db, T: HistoryTracker<R::Op>> LogSource<R::Op> for Processor<R, D, T>
where
    R::Error: std::error::Error + 'static,
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
