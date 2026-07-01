//! Backend-agnostic end-to-end [`Processor`] suite, for the driver crates to
//! run against their real [`Db`].
//!
//! The reducer under test is a per-key **max register** (a grow-only CRDT): an
//! op carries `(key, value)` and the stored value only ever moves up, via a
//! `MAX`-guarded upsert. That is enough to exercise the whole `prepare` →
//! `apply` → `post_apply` pipeline, the op-log tracker, and the HLC observe —
//! and, crucially, the idempotency contract: re-ingesting a `(peer_id,
//! entry_idx)` must roll the entire batch back and apply nothing.
//!
//! [`run_max_register_suite`] is generic over `<D: Db>`, so the exact same
//! assertions run against any backend. Today only the SQLite driver implements
//! [`Db`], so there is one caller (in `ubiquisync-sqlite`'s tests); when the
//! Postgres driver lands, a second test that hands the suite a `PgDb` is all it
//! takes.

use std::io::BufRead;

use async_trait::async_trait;
use ubiquisync_core::{
    codec::{
        CodecError, DecodedEntry, EntryBufferReader, EntryBufferWriter, IndexableOp, Op,
        OpIndexEntry,
    },
    hlc::Timestamp,
    log_entry::LogEntry,
    sync::{LogProcessor, LogSource, PullSynchronizer, SyncError},
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
/// Generic over the backend so each driver crate's tests can reuse it verbatim
/// — call it with a freshly opened, empty database. Returned as a future rather
/// than blocking internally, so each driver runs it inside its own runtime (a
/// trivial poll for synchronous backends, a real executor for async ones).
pub async fn run_max_register_suite<D: Db>(db: D) {
    let mut processor: MaxProcessor<D> = Processor::open(MaxRegister::new("reg"), db, PREFIX)
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

    // Replaying a byte-identical, already-applied entry (the real dedup case)
    // also surfaces the violation rather than silently re-applying.
    let err = processor
        .process_one(&PEER, 1, &entry(b"x", 3, 1_700_000_000_001, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProcessorError::Db(DbError::UniqueViolation)),
        "identical replay surfaces as a unique violation, got {err:?}"
    );

    // A different key is an independent register.
    let r = processor
        .process_one(&PEER, 4, &entry(b"y", 7, 1_700_000_000_005, None))
        .await
        .unwrap();
    assert_eq!(r, 7, "distinct key has its own register");
}

// ── End-to-end sync harness ─────────────────────────────────────────────────

/// A fixed, in-memory stream of one peer's decoded entries, standing in for a
/// real segment store so the [`Processor`] can be driven through
/// [`PullSynchronizer`].
struct MockSource {
    peer: Uuid,
    entries: Vec<DecodedEntry<MaxOp>>,
}

impl LogSource<MaxOp> for MockSource {
    fn list_peers(&self) -> Vec<Uuid> {
        vec![self.peer]
    }

    fn read_entries(
        &self,
        peer: &Uuid,
        start_entry_idx: u64,
    ) -> Result<Vec<(u64, DecodedEntry<MaxOp>)>, SyncError> {
        if peer != &self.peer {
            return Ok(vec![]);
        }
        Ok(self
            .entries
            .iter()
            .cloned()
            .enumerate()
            .skip(usize::try_from(start_entry_idx).expect("cursor exceeds usize"))
            .map(|(i, e)| (i as u64, e))
            .collect())
    }
}

/// Drives a processor through [`PullSynchronizer`] end to end against `db`,
/// exercising the [`LogProcessor`] implementation: real entries apply, expunged
/// markers are recorded (not applied) yet still advance the cursor past the gap,
/// the cursor is derived from what the tracker recorded rather than stored
/// separately, and a second pass re-delivers nothing because the cursor already
/// sits at the stream end.
///
/// Generic over the backend like [`run_max_register_suite`]; call it with a
/// freshly opened, empty database.
pub async fn run_pull_sync_suite<D: Db>(db: D) {
    let mut processor: MaxProcessor<D> = Processor::open(MaxRegister::new("reg"), db, PREFIX)
        .await
        .unwrap();

    // Stream: two real entries, an expunged marker at index 2, then one more
    // real entry. The second real entry is server-attributed.
    let source = MockSource {
        peer: PEER,
        entries: vec![
            DecodedEntry::LogEntry(entry(b"x", 5, 1_700_000_000_000, None)),
            DecodedEntry::LogEntry(entry(b"x", 9, 1_700_000_000_001, Some(USER))),
            DecodedEntry::Expunged(blake3::hash(b"gone")),
            DecodedEntry::LogEntry(entry(b"y", 4, 1_700_000_000_002, None)),
        ],
    };

    // A never-seen peer starts at cursor 0.
    assert_eq!(processor.get_peer_cursor(&PEER).await.unwrap(), 0);

    let result = PullSynchronizer::new(&source, None)
        .sync(&mut processor)
        .await
        .unwrap();
    assert_eq!(
        result.entries_applied, 3,
        "3 real entries applied; the expunged marker is not an apply"
    );
    // Cursor sits one past the last stream index (3), so it advanced over the
    // expunged gap at index 2 as well.
    assert_eq!(
        processor.get_peer_cursor(&PEER).await.unwrap(),
        4,
        "cursor advanced past every slot, expunged gap included"
    );
    // Four op-log rows: three entries plus the expunged marker occupying its
    // index. Attribution landed only on the server-attested entry.
    assert_eq!(oplog_row_count(processor.db()).await, 4);
    assert_eq!(oplog_server_user_id(processor.db(), 1).await, Some(USER));
    assert_eq!(
        oplog_server_user_id(processor.db(), 2).await,
        None,
        "expunged marker carries no attribution"
    );

    // A second pass reads from the persisted cursor (4), finds nothing new, and
    // applies nothing — no re-delivery, no duplicate rows.
    let result = PullSynchronizer::new(&source, None)
        .sync(&mut processor)
        .await
        .unwrap();
    assert_eq!(result.entries_applied, 0, "second sync re-delivers nothing");
    assert_eq!(
        oplog_row_count(processor.db()).await,
        4,
        "re-sync wrote no duplicate rows"
    );

    // Dedup at the `LogProcessor` method the engine actually calls: handing
    // `apply_entry` an already-recorded index fails the op-log PK and rolls the
    // whole batch back — the safety net if a duplicate ever reaches apply. The
    // `y` register (index 3, value 4) must be untouched by the rolled-back apply.
    let err = processor
        .apply_entry(&PEER, 3, &entry(b"y", 999, 1_700_000_000_009, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProcessorError::Db(DbError::UniqueViolation)),
        "re-applying a recorded index is rejected, got {err:?}"
    );
    assert_eq!(
        oplog_row_count(processor.db()).await,
        4,
        "rejected duplicate added no op-log row"
    );

    // Incremental resume through the real derived cursor: the peer's stream
    // grows by two entries and a fresh pass applies only those, resuming from
    // the persisted cursor (4) with no mock cursor anywhere in the loop.
    let grown = MockSource {
        peer: PEER,
        entries: {
            let mut e = source.entries.clone();
            e.push(DecodedEntry::LogEntry(entry(b"x", 12, 1_700_000_000_003, None)));
            e.push(DecodedEntry::LogEntry(entry(b"z", 1, 1_700_000_000_004, None)));
            e
        },
    };
    let result = PullSynchronizer::new(&grown, None)
        .sync(&mut processor)
        .await
        .unwrap();
    assert_eq!(result.entries_applied, 2, "only the two appended entries apply");
    assert_eq!(
        processor.get_peer_cursor(&PEER).await.unwrap(),
        6,
        "cursor resumed from 4 and advanced past the two new entries"
    );
    assert_eq!(oplog_row_count(processor.db()).await, 6);
}
