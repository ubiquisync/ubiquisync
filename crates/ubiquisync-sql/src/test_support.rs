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

use std::future::Future;
use std::io::BufRead;
use std::pin::pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use async_trait::async_trait;
use ubiquisync_core::{
    codec::{CodecError, EntryBufferReader, EntryBufferWriter, IndexableOp, Op, OpIndexEntry},
    hlc::Timestamp,
    log_entry::LogEntry,
    uuid::Uuid,
};

use crate::{
    db::{Db, DbBatch, DbError, DbStatementResult, DbType, DbValue, StmtId, ValueBinder},
    processor::Processor,
    reducer::Reducer,
    tracker::LogIndexTracker,
    util::quote_ident,
};

/// Minimal executor: every future in these crates resolves without ever
/// yielding (the bodies do no real `.await`), so polling once in a loop with a
/// no-op waker is sufficient — no async runtime dependency needed. Mirrors the
/// helper the SQLite crate's own tests use.
fn block_on<F: Future>(fut: F) -> F::Output {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

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
        let sql = format!(
            "INSERT INTO {tbl} (k, v) VALUES ({k}, {v}) \
             ON CONFLICT(k) DO UPDATE SET v = {max}(v, EXCLUDED.v) RETURNING v",
            tbl = self.table
        );
        Ok(batch.add_statement(&sql, &binder.values()))
    }

    fn post_apply(
        &self,
        apply_state: StmtId,
        batch_result: &[DbStatementResult],
    ) -> Result<i64, DbError> {
        batch_result[apply_state.0].rows[0].get_i64(0)
    }
}

// ── Harness ─────────────────────────────────────────────────────────────────

type MaxProcessor<D> = Processor<MaxRegister, D, LogIndexTracker<MaxOp>>;

const CLIENT: Uuid = [7u8; 16];

fn entry(key: &[u8], value: i64, millis: u64) -> LogEntry<MaxOp> {
    LogEntry {
        user_id: None,
        // A past wall component is always within the HLC skew bound.
        timestamp: Timestamp::from_parts(millis, 0),
        op: MaxOp {
            key: key.to_vec(),
            value,
        },
    }
}

/// Drives one processor through the max-register scenarios against `db`.
/// Generic over the backend so each driver crate's tests can reuse it verbatim
/// — call it with a freshly opened, empty database.
pub fn run_max_register_suite<D: Db>(db: D) {
    let mut processor: MaxProcessor<D> =
        block_on(Processor::open(MaxRegister::new("reg"), db, "app")).unwrap();

    // First write seeds the register.
    let r = block_on(processor.process_one(&CLIENT, 0, &entry(b"x", 5, 1_700_000_000_000))).unwrap();
    assert_eq!(r, Some(5), "first write sets the value");

    // A smaller value loses the max merge but is still a real (non-duplicate)
    // apply, so it returns the unchanged max.
    let r = block_on(processor.process_one(&CLIENT, 1, &entry(b"x", 3, 1_700_000_000_001))).unwrap();
    assert_eq!(r, Some(5), "smaller value does not lower the register");

    // A larger value advances it.
    let r = block_on(processor.process_one(&CLIENT, 2, &entry(b"x", 9, 1_700_000_000_002))).unwrap();
    assert_eq!(r, Some(9), "larger value raises the register");

    // Re-ingesting (CLIENT, 0) is a duplicate: the op-log PK conflict must roll
    // the *whole* batch back, including this op's would-be max bump to 100.
    let r =
        block_on(processor.process_one(&CLIENT, 0, &entry(b"x", 100, 1_700_000_000_003))).unwrap();
    assert_eq!(r, None, "duplicate (client_id, client_idx) is skipped");

    // Prove the duplicate applied nothing: the register is still 9, not 100.
    let r = block_on(processor.process_one(&CLIENT, 3, &entry(b"x", 1, 1_700_000_000_004))).unwrap();
    assert_eq!(r, Some(9), "rolled-back duplicate left the register at 9");

    // A different key is an independent register.
    let r = block_on(processor.process_one(&CLIENT, 4, &entry(b"y", 7, 1_700_000_000_005))).unwrap();
    assert_eq!(r, Some(7), "distinct key has its own register");
}
