//! Op → SQL translation: the [`Reducer`] trait a data domain implements to turn
//! each of its ops into the backend writes that materialize it.

use ubiquisync_core::hlc::Timestamp;

use crate::{
    db::{Db, DbBatch, DbStatementResult},
    op::OpCodec,
};

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
///    `RETURNING` rows finally exist.
#[async_trait::async_trait]
pub trait Reducer: Send + Sync {
    /// The op vocabulary this reducer materializes (e.g. the table op enum).
    type Op: Send + Sync;
    type ReadState: Send + Sync;
    /// Carried from [`apply`](Reducer::apply) to
    /// [`post_apply`](Reducer::post_apply): the `StmtId`s of the emitted
    /// statements plus any op-derived data needed to build the event.
    type ApplyState: Send + Sync;
    /// Error surfaced from any phase.
    type Error: core::error::Error + Send + Sync + 'static;

    fn codec(&self) -> &dyn OpCodec<Self::Op>;

    /// Reconcile the schema needed by `op` (create/alter tables, refresh any
    /// cache) and read whatever `apply` will need, returning it as a
    /// [`ReadState`](Reducer::ReadState). Runs outside the batch — DDL is
    /// additive and safe to commit on its own, and hoisting reads here is what
    /// keeps `apply` pure.
    async fn prepare(&self, db: &dyn Db, op: &Self::Op) -> Result<Self::ReadState, Self::Error>;

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

    /// `batch_result` holds the
    /// whole batch's per-statement results in add order; locate this op's
    /// `RETURNING` rows via the `StmtId`s stored in `apply_state`.
    fn post_apply(
        &self,
        apply_state: Self::ApplyState,
        batch_result: &[DbStatementResult],
    ) -> Result<(), Self::Error>;
}
