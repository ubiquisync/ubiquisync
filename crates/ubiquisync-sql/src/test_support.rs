//! Backend-agnostic end-to-end [`Processor`] suite, for the driver crates to
//! run against their real [`Db`].
//!
//! The reducer under test is a per-key **max register** (a grow-only CRDT): an
//! op carries `(key, value)` and the stored value only ever moves up, via a
//! `MAX`-guarded upsert. That is enough to exercise the whole `prepare` →
//! `apply` → `post_apply` pipeline, the op-log tracker, and the HLC observe —
//! and, crucially, the idempotency contract: re-ingesting a `(client_id,
//! client_idx)` must roll the entire batch back and apply nothing.
//!
//! [`run_max_register_suite`] is generic over `<D: Db>`, so the exact same
//! assertions run against any backend. Today only the SQLite driver implements
//! [`Db`], so there is one caller (in `ubiquisync-sqlite`'s tests); when the
//! Postgres driver lands, a second test that hands the suite a `PgDb` is all it
//! takes.

use std::io::BufRead;

use async_trait::async_trait;
use ubiquisync_core::{
    codec::{CodecError, EntryBufferReader, EntryBufferWriter, IndexableOp, Op, OpIndexEntry},
    hlc::Timestamp,
    log_entry::LogEntry,
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

const CLIENT: Uuid = [7u8; 16];
const USER: Uuid = [9u8; 16];
const PREFIX: &str = "app";

fn entry(key: &[u8], value: i64, millis: u64, user_id: Option<Uuid>) -> LogEntry<MaxOp> {
    LogEntry {
        user_id,
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

async fn oplog_user_id<D: Db>(db: &D, client_idx: u64) -> Option<Uuid> {
    let mut binder = ValueBinder::new(db.dialect());
    let idx = binder.bind_next(DbValue::from_u64(client_idx).unwrap());
    let sql = format!(
        "SELECT user_id FROM {} WHERE client_idx = {idx}",
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
        .process_one(&CLIENT, 0, &entry(b"x", 5, 1_700_000_000_000, None))
        .await
        .unwrap();
    assert_eq!(r, 5, "first write sets the value");

    // A smaller value loses the max merge but is still a real (non-duplicate)
    // apply, so it returns the unchanged max.
    let r = processor
        .process_one(&CLIENT, 1, &entry(b"x", 3, 1_700_000_000_001, None))
        .await
        .unwrap();
    assert_eq!(r, 5, "smaller value does not lower the register");

    // A larger value advances it. This entry carries a user id, so its op-log
    // row exercises the `Some(user)` binding path.
    let r = processor
        .process_one(&CLIENT, 2, &entry(b"x", 9, 1_700_000_000_002, Some(USER)))
        .await
        .unwrap();
    assert_eq!(r, 9, "larger value raises the register");
    assert_eq!(
        oplog_user_id(processor.db(), 2).await,
        Some(USER),
        "attributed entry stores its user id"
    );
    assert_eq!(
        oplog_user_id(processor.db(), 0).await,
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

    // Re-ingesting (CLIENT, 0) is a duplicate. Its timestamp (…003) is *higher*
    // than the persisted clock and its value (100) would raise the register, so
    // a missed rollback would be visible in all three of: the register, the
    // op-log row count, and the persisted clock.
    let err = processor
        .process_one(&CLIENT, 0, &entry(b"x", 100, 1_700_000_000_003, None))
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
        .process_one(&CLIENT, 3, &entry(b"x", 1, 1_700_000_000_004, None))
        .await
        .unwrap();
    assert_eq!(r, 9, "rolled-back duplicate left the register at 9");

    // Replaying a byte-identical, already-applied entry (the real dedup case)
    // also surfaces the violation rather than silently re-applying.
    let err = processor
        .process_one(&CLIENT, 1, &entry(b"x", 3, 1_700_000_000_001, None))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProcessorError::Db(DbError::UniqueViolation)),
        "identical replay surfaces as a unique violation, got {err:?}"
    );

    // A different key is an independent register.
    let r = processor
        .process_one(&CLIENT, 4, &entry(b"y", 7, 1_700_000_000_005, None))
        .await
        .unwrap();
    assert_eq!(r, 7, "distinct key has its own register");
}
