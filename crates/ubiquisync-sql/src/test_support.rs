//! Backend-agnostic end-to-end [`Processor`] suite, for the driver crates to
//! run against their real [`Db`].
//!
//! The reducer under test is a per-key **max register** (a grow-only CRDT): an
//! op carries `(key, value)` and the stored value only ever moves up, via a
//! `MAX`-guarded upsert. That is enough to exercise the whole `prepare` →
//! `apply` → `post_apply` pipeline, the op-log tracker, and the HLC observe.
//!
//! [`run_max_register_suite`] is generic over `<D: Db>`, so the exact same
//! assertions run against any backend. Today only the SQLite driver implements
//! [`Db`], so there is one caller (in `ubiquisync-sqlite`'s tests); when the
//! Postgres driver lands, a second test that hands the suite a `PgDb` is all it
//! takes.

use std::io::BufRead;

use async_trait::async_trait;
use futures::StreamExt;
use ubiquisync_core::{
    codec::{
        CodecError, DecodedEntry, EntryBufferReader, EntryBufferWriter, IndexableOp, Op,
        OpIndexEntry,
    },
    hlc::Timestamp,
    log_entry::LogEntry,
    sync::{CursorsEvent, HasCursors, LogProcessor, LogSource},
    uuid::Uuid,
};

use crate::{
    db::{Db, DbBatch, DbError, DbStatementResult, DbType, DbValue, StmtId, ValueBinder},
    processor::{Processor, ProcessorError},
    reducer::Reducer,
    tracker::LogIndexTracker,
    util::quote_ident,
};

// ── Mock op vocabulary ──────────────────────────────────────────────────────

const TAG_MAX: u8 = 1;

/// "Set this key's register to at least `value`." The only op the mock reducer
/// understands.
#[derive(Clone)]
struct MaxOp {
    key: Vec<u8>,
    value: i64,
}

impl Op for MaxOp {
    fn decode<R: BufRead>(_tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError> {
        let key = r.read_blob()?;
        let value = r.read_zigzag()?;
        Ok(MaxOp { key, value })
    }

    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError> {
        w.write_byte(TAG_MAX);
        w.write_blob(&self.key);
        w.write_zigzag(self.value);
        Ok(())
    }
}

impl IndexableOp for MaxOp {
    fn to_index_entry(&self) -> Result<OpIndexEntry, CodecError> {
        Ok(OpIndexEntry {
            tag: TAG_MAX,
            key: self.key.clone(),
            value: self.value.to_le_bytes().to_vec(),
        })
    }

    fn from_index_parts(_tag: u8, key: &[u8], value: &[u8]) -> Result<Self, CodecError> {
        let value = i64::from_le_bytes(value.try_into().map_err(|_| CodecError::UnexpectedEof)?);
        Ok(MaxOp {
            key: key.to_vec(),
            value,
        })
    }
}

// ── Mock reducer: a per-key max register ────────────────────────────────────

/// Materializes [`MaxOp`]s into a `(k, v)` table where `v` only ever grows.
struct MaxRegister {
    /// Already-quoted data-table name.
    table: String,
}

impl MaxRegister {
    fn new(name: &str) -> Self {
        Self {
            table: quote_ident(name),
        }
    }
}

#[async_trait(?Send)]
impl Reducer for MaxRegister {
    type Op = MaxOp;
    type ReadState = ();
    /// The upsert's id, so `post_apply` can find its `RETURNING` row.
    type ApplyState = StmtId;
    /// The register's value after the merge.
    type Event = i64;
    type Error = DbError;

    async fn prepare(&mut self, db: &dyn Db, _op: &MaxOp) -> Result<(), DbError> {
        let int_type = DbType::Integer.sql_type(db.dialect());
        let blob_type = DbType::Blob.sql_type(db.dialect());
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} (k {blob_type} PRIMARY KEY, v {int_type} NOT NULL)",
            self.table
        );
        db.exec(&sql, &[]).await?;
        Ok(())
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        _timestamp: Timestamp,
        op: &MaxOp,
        _read: (),
    ) -> Result<StmtId, DbError> {
        let mut binder = ValueBinder::new(batch.dialect());
        let k = binder.bind_next(DbValue::Blob(op.key.clone()));
        let v = binder.bind_next(DbValue::Integer(op.value));
        let max = batch.dialect().scalar_max();
        // COALESCE the stored side: SQLite `MAX` returns NULL on a NULL arg
        // while Postgres `GREATEST` ignores NULLs, so the wrapper is what makes
        // the merge agree across dialects (mirrors `SqlHlcStorage`).
        let sql = format!(
            "INSERT INTO {tbl} (k, v) VALUES ({k}, {v}) \
             ON CONFLICT(k) DO UPDATE SET v = {max}(COALESCE(v, 0), EXCLUDED.v) RETURNING v",
            tbl = self.table
        );
        Ok(batch.add_statement(&sql, &binder.values()))
    }

    fn post_apply(
        &self,
        apply_state: StmtId,
        batch_result: &[DbStatementResult],
    ) -> Result<i64, DbError> {
        // Safe to index `rows[0]` only because this upsert's `DO UPDATE` is
        // unconditional, so `RETURNING` always yields a row. A reducer with a
        // guarded upsert must handle an empty result instead.
        batch_result[apply_state.0].rows[0].get_i64(0)
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

type MaxProcessor<D> = Processor<MaxRegister, D, LogIndexTracker<MaxOp>>;

const PEER: Uuid = [7u8; 16];
const USER: Uuid = [9u8; 16];
const NODE: Uuid = [1u8; 16];
const PREFIX: &str = "app";

fn entry(key: &[u8], value: i64, millis: u64, server_user_id: Option<Uuid>) -> LogEntry<MaxOp> {
    LogEntry {
        server_user_id,
        // A past wall component is always within the HLC skew bound.
        timestamp: Timestamp::from_parts(millis, 0),
        op: MaxOp {
            key: key.to_vec(),
            value,
        },
    }
}

async fn oplog_row_count<D: Db>(db: &D) -> i64 {
    let sql = format!(
        "SELECT COUNT(*) FROM {}",
        quote_ident(&format!("{PREFIX}__oplog"))
    );
    db.query(&sql, &[]).await.unwrap()[0].get_i64(0).unwrap()
}

async fn oplog_server_user_id<D: Db>(db: &D, entry_idx: u64) -> Option<Uuid> {
    let mut binder = ValueBinder::new(db.dialect());
    let idx = binder.bind_next(DbValue::from_u64(entry_idx).unwrap());
    let sql = format!(
        "SELECT server_user_id FROM {} WHERE entry_idx = {idx}",
        quote_ident(&format!("{PREFIX}__oplog"))
    );
    db.query(&sql, &binder.values()).await.unwrap()[0]
        .get_optional_uuid(0)
        .unwrap()
}

/// The current value of a max register `key`, read straight from the data table.
async fn register_value<D: Db>(db: &D, key: &[u8]) -> i64 {
    let mut binder = ValueBinder::new(db.dialect());
    let k = binder.bind_next(DbValue::Blob(key.to_vec()));
    let sql = format!("SELECT v FROM {} WHERE k = {k}", quote_ident("reg"));
    db.query(&sql, &binder.values()).await.unwrap()[0]
        .get_i64(0)
        .unwrap()
}

/// The durably persisted HLC clock — the value seeded on the next `open`.
async fn clock_register<D: Db>(db: &D) -> u64 {
    let sql = format!(
        "SELECT ts FROM {} WHERE id = 1",
        quote_ident(&format!("{PREFIX}__hlc"))
    );
    db.query(&sql, &[]).await.unwrap()[0].get_u64(0).unwrap()
}

/// Drives one processor through the max-register scenarios against `db`.
///
/// Exercises `process_one` directly — the raw ingest path, which errors on a
/// duplicate `(peer, index)` rather than dropping it (dedup is the job of
/// [`apply`](LogProcessor::apply), covered by [`run_pull_sync_suite`]).
///
/// Generic over the backend so each driver crate's tests can reuse it verbatim.
pub async fn run_max_register_suite<D: Db>(db: D) {
    let processor: MaxProcessor<D> = Processor::open(MaxRegister::new("reg"), db, PREFIX, NODE)
        .await
        .unwrap();

    // First write seeds the register.
    let r = processor
        .process_one(&PEER, 0, &entry(b"x", 5, 1_700_000_000_000, None))
        .await
        .unwrap();
    assert_eq!(r, 5, "first write sets the value");

    // A smaller value loses the max merge but is still a real (non-duplicate)
    // apply, so it returns the unchanged max.
    let r = processor
        .process_one(&PEER, 1, &entry(b"x", 3, 1_700_000_000_001, None))
        .await
        .unwrap();
    assert_eq!(r, 5, "smaller value does not lower the register");

    // A larger value advances it. This entry carries a user id, so its op-log
    // row exercises the `Some(user)` binding path.
    let r = processor
        .process_one(&PEER, 2, &entry(b"x", 9, 1_700_000_000_002, Some(USER)))
        .await
        .unwrap();
    assert_eq!(r, 9, "larger value raises the register");
    assert_eq!(
        oplog_server_user_id(processor.db(), 2).await,
        Some(USER),
        "attributed entry stores its user id"
    );
    assert_eq!(
        oplog_server_user_id(processor.db(), 0).await,
        None,
        "unattributed entry stores NULL user id"
    );

    // State the three committed entries left behind, for the rollback checks.
    let committed_rows = oplog_row_count(processor.db()).await;
    let committed_clock = clock_register(processor.db()).await;
    assert_eq!(committed_rows, 3);
    assert_eq!(
        committed_clock,
        Timestamp::from_parts(1_700_000_000_002, 0).raw()
    );

    // Re-ingesting (PEER, 0) is a duplicate. Its timestamp (…003) is *higher*
    // than the persisted clock and its value (100) would raise the register, so
    // a missed rollback would be visible in all three of: the register, the
    // op-log row count, and the persisted clock.
    let err = processor
        .process_one(&PEER, 0, &entry(b"x", 100, 1_700_000_000_003, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProcessorError::Db(DbError::UniqueViolation)),
        "duplicate surfaces as a unique violation, got {err:?}"
    );
    assert_eq!(
        oplog_row_count(processor.db()).await,
        committed_rows,
        "rolled-back duplicate added no op-log row"
    );
    assert_eq!(
        clock_register(processor.db()).await,
        committed_clock,
        "rolled-back observe did not advance the persisted clock"
    );

    // Prove the duplicate applied nothing at the data layer: still 9, not 100.
    let r = processor
        .process_one(&PEER, 3, &entry(b"x", 1, 1_700_000_000_004, None))
        .await
        .unwrap();
    assert_eq!(r, 9, "rolled-back duplicate left the register at 9");

    // A different key is an independent register.
    let r = processor
        .process_one(&PEER, 4, &entry(b"y", 7, 1_700_000_000_005, None))
        .await
        .unwrap();
    assert_eq!(r, 7, "distinct key has its own register");
}

/// Drives a processor through the [`Replica`](ubiquisync_core::sync::Replica)
/// faces against `db`: [`apply`](LogProcessor::apply) ingests real entries and
/// expunged markers (advancing the cursor over the gap), re-delivery is a silent
/// no-op, [`read_since`](LogSource::read_since) reconstructs the stream from the
/// op-log, and [`watch_cursors`](HasCursors::watch_cursors) reports progress.
///
/// Generic over the backend like [`run_max_register_suite`]; call it with a
/// freshly opened, empty database.
pub async fn run_pull_sync_suite<D: Db>(db: D) {
    let processor: MaxProcessor<D> = Processor::open(MaxRegister::new("reg"), db, PREFIX, NODE)
        .await
        .unwrap();

    // A never-seen peer has no cursor yet.
    assert!(!processor.cursors().await.unwrap().contains_key(&PEER));

    // Stream: two real entries, an expunged marker at index 2, then one more
    // real entry. Index 1 is server-attributed.
    let stream = [
        DecodedEntry::LogEntry(entry(b"x", 5, 1_700_000_000_000, None)),
        DecodedEntry::LogEntry(entry(b"x", 9, 1_700_000_000_001, Some(USER))),
        DecodedEntry::Expunged(blake3::hash(b"gone")),
        DecodedEntry::LogEntry(entry(b"y", 4, 1_700_000_000_002, None)),
    ];
    for (idx, e) in stream.iter().cloned().enumerate() {
        assert!(
            processor.apply(PEER, idx as u64, e).await.unwrap().new,
            "fresh slot {idx} applies"
        );
    }

    // Cursor sits one past the last slot (3), so it advanced over the expunged
    // gap at index 2 as well. Four op-log rows: three entries plus the marker;
    // attribution landed only on the server-attested entry.
    assert_eq!(processor.cursors().await.unwrap().get(&PEER).copied(), Some(4));
    assert_eq!(oplog_row_count(processor.db()).await, 4);
    assert_eq!(oplog_server_user_id(processor.db(), 1).await, Some(USER));
    assert_eq!(
        oplog_server_user_id(processor.db(), 2).await,
        None,
        "expunged marker carries no attribution"
    );

    // read_since reconstructs the whole stream from the op-log, marker included.
    let read = processor.read_since(PEER, 0).await.unwrap();
    assert_eq!(
        read.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(
        matches!(read[2].1, DecodedEntry::Expunged(_)),
        "index 2 round-trips as the marker"
    );
    match &read[1].1 {
        DecodedEntry::LogEntry(e) => {
            assert_eq!(e.server_user_id, Some(USER));
            assert_eq!(e.op.value, 9);
            assert_eq!(e.op.key, b"x");
        }
        _ => panic!("index 1 should reconstruct as a real entry"),
    }

    // Idempotent re-delivery: an already-applied index is a silent no-op
    // (new == false), not an error, and changes nothing.
    let redelivery = processor
        .apply(
            PEER,
            1,
            DecodedEntry::LogEntry(entry(b"x", 999, 1_700_000_000_009, None)),
        )
        .await
        .unwrap();
    assert!(!redelivery.new, "re-delivered index is dropped");
    assert_eq!(
        oplog_row_count(processor.db()).await,
        4,
        "dropped re-delivery added no row"
    );
    assert_eq!(
        register_value(processor.db(), b"x").await,
        9,
        "dropped re-delivery left the register at 9, not 999"
    );

    // watch_cursors: first event is a snapshot; a later apply broadcasts an
    // advance delta.
    let mut watch = processor.watch_cursors();
    match watch.next().await {
        Some(CursorsEvent::Snapshot(c)) => assert_eq!(c.get(&PEER).copied(), Some(4)),
        other => panic!("expected a snapshot first, got {other:?}"),
    }
    assert!(
        processor
            .apply(
                PEER,
                4,
                DecodedEntry::LogEntry(entry(b"x", 12, 1_700_000_000_003, None)),
            )
            .await
            .unwrap()
            .new
    );
    match watch.next().await {
        Some(CursorsEvent::Advanced(c)) => assert_eq!(c.get(&PEER).copied(), Some(5)),
        other => panic!("expected an advance, got {other:?}"),
    }

    // Resume: the next new index applies; cursor and row count track it.
    assert!(
        processor
            .apply(
                PEER,
                5,
                DecodedEntry::LogEntry(entry(b"z", 1, 1_700_000_000_004, None)),
            )
            .await
            .unwrap()
            .new
    );
    assert_eq!(processor.cursors().await.unwrap().get(&PEER).copied(), Some(6));
    assert_eq!(oplog_row_count(processor.db()).await, 6);
}
