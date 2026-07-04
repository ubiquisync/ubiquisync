//! Op → SQL translation: the [`Reducer`] trait a data domain implements to turn
//! each of its ops into the backend writes that materialize it.
//!
//! Kept as its own module so the [ingestion processor](crate::processor) can
//! drive any reducer generically. See [`Reducer`] for the three-phase
//! prepare/apply/post_apply contract that lets one op map onto every backend.

use ubiquisync_core::hlc::Timestamp;

use crate::db::{Db, DbBatch, DbStatementResult};

/// Translates a single op into the SQL writes that materialize it, in three
/// phases so the work maps onto every backend — including ones with no
/// interactive transaction (e.g. D1's `batch()`):
///
/// 1. [`prepare`](Reducer::prepare) runs *before* the batch. It is the only
///    phase allowed to read or issue DDL, and it returns the
///    [`ReadState`](Reducer::ReadState) `apply` needs — so every read is hoisted
///    out of the batch.
/// 2. [`apply`](Reducer::apply) emits the op's mutation statements into the open
///    batch. It is pure and read-free (it consumes the `ReadState`), so the
///    batch stays a flat, declarative statement list.
/// 3. [`post_apply`](Reducer::post_apply) runs *after* the batch commits, when
///    `RETURNING` rows finally exist, and turns the results into zero or more
///    events — empty when the op changed nothing observable.
/// # Idempotency
///
/// Whether a reducer must apply idempotently is not its own choice — it depends
/// on the [`LogTracker`](crate::tracker::LogTracker) it is paired with. Behind a
/// tracker that rejects a repeated `(peer_id, entry_idx)` (e.g.
/// [`LogIndexTracker`](crate::tracker::LogIndexTracker)) a redundant apply rolls
/// back, so the reducer need not be idempotent; behind one that does not reject,
/// it must be. See the tracker's
/// [duplicate-rejection contract](crate::tracker::LogTracker#duplicate-rejection).
#[async_trait::async_trait]
pub trait Reducer: Send {
    /// The op vocabulary this reducer materializes (e.g. the table op enum).
    type Op: Send + Sync;
    /// Data read in [`prepare`](Reducer::prepare) and consumed by
    /// [`apply`](Reducer::apply): e.g. a card's prior FSRS state, or its full
    /// review history when an out-of-order op forces a recompute. `()` when
    /// `apply` needs nothing read.
    type ReadState;
    /// Carried from [`apply`](Reducer::apply) to
    /// [`post_apply`](Reducer::post_apply): the `StmtId`s of the emitted
    /// statements plus any op-derived data needed to build the event.
    type ApplyState: Send;
    /// The change event produced for an applied op, for downstream observers.
    type Event: Send + Clone;
    /// Error surfaced from any phase.
    type Error;

    /// Reconcile the schema needed by `op` (create/alter tables, refresh any
    /// cache) and read whatever `apply` will need, returning it as a
    /// [`ReadState`](Reducer::ReadState). Runs outside the batch — DDL is
    /// additive and safe to commit on its own, and hoisting reads here is what
    /// keeps `apply` pure.
    async fn prepare(&mut self, db: &dyn Db, op: &Self::Op)
    -> Result<Self::ReadState, Self::Error>;

    /// Emit the statements that materialize `op` at `timestamp` into `batch`,
    /// using only `op`, the cached schema, and `read`. Read-free, so it stays
    /// expressible as a declarative batch. The returned
    /// [`ApplyState`](Reducer::ApplyState) is provisional until `batch` commits.
    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        op: &Self::Op,
        read: Self::ReadState,
    ) -> Result<Self::ApplyState, Self::Error>;

    /// Build the events once the batch has committed — empty when the op changed
    /// nothing observable (e.g. an upsert that lost every column's LWW, or a
    /// write to a table with no observer-facing name), usually one, or several
    /// when a single op has multiple logical effects. `batch_result` holds the
    /// whole batch's per-statement results in add order; locate this op's
    /// `RETURNING` rows via the `StmtId`s stored in `apply_state`.
    fn post_apply(
        &self,
        apply_state: Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<Vec<Self::Event>, Self::Error>;
}
