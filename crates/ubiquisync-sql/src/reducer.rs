//! Op → SQL translation: the [`Reducer`] trait a data domain implements to turn
//! each of its ops into the backend writes that materialize it.

use thiserror::Error;
use ubiquisync_core::{
    hlc::Timestamp,
    ids::{ContainerId, LogId},
};

use crate::{
    db::{Db, DbBatch, DbError, DbStatementResult},
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
pub trait Reducer: Send + Sync + 'static {
    /// The op vocabulary this reducer materializes (e.g. the table op enum).
    type Op: Send + Sync;
    type ReadState: Send + Sync;
    /// Carried from [`apply`](Reducer::apply) to
    /// [`post_apply`](Reducer::post_apply): the `StmtId`s of the emitted
    /// statements plus any op-derived data needed to build the event.
    type ApplyState: Send + Sync;

    fn codec(&self) -> &dyn OpCodec<Self::Op>;

    /// Reconcile the schema needed by `op` (create/alter tables, refresh any
    /// cache) and read whatever `apply` will need, returning it as a
    /// [`ReadState`](Reducer::ReadState). Runs outside the batch — DDL is
    /// additive and safe to commit on its own, and hoisting reads here is what
    /// keeps `apply` pure.
    async fn prepare(&self, db: &dyn Db, op: &Self::Op) -> Result<Self::ReadState, PrepareError>;

    /// Emit the statements that materialize `op` at `timestamp` into `batch`,
    /// using only `op`, the cached schema, and `read`. Read-free, so it stays
    /// expressible as a declarative batch. The returned
    /// [`ApplyState`](Reducer::ApplyState) is provisional until `batch` commits.
    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        // TODO server_attested_user_id: Option<Uuid>,
        op: &Self::Op,
        read: Self::ReadState,
    ) -> Result<Self::ApplyState, ApplyError>;

    /// `batch_result` holds the
    /// whole batch's per-statement results in add order; locate this op's
    /// `RETURNING` rows via the `StmtId`s stored in `apply_state`.
    fn post_apply(&self, apply_state: Self::ApplyState, batch_result: &[DbStatementResult]);
}

#[derive(Error, Debug)]
pub enum PrepareError {
    /// A database related error, may be transient or a software bug.
    #[error("db error: {0}")]
    Db(#[from] DbError),
    /// Some internal error which is likely a bug, but in some edge cases could be transient.
    #[error("internal: {0}")]
    Internal(String),
    /// This operation depends on other specified logs before it can be committed.
    /// The replica should keep track of this and only commit this operation after
    /// those dependencies have been committed.
    #[error("needs dependencies: {0:?}")]
    NeedsDeps(Vec<PeerDependency>),
    /// Indicates that the reducer state needs to be rebuilt.
    /// The replica should drop all reducer state and re-apply all operations.
    #[error("needs rebuild: {0:?}")]
    NeedsRebuild(RebuildScope),
    /// Indicates that the operation should be skipped because it is invalid.
    /// This return code will cause the replica to skip calling `apply` but still
    /// mark the op as committed.
    #[error("invalid op: {0}")]
    InvalidOp(String),
    /// This should almost never be used, but it instructs the replica to stop committing this log
    /// for good.
    #[error("frozen: {0}")]
    Frozen(String),
}

#[derive(Error, Debug)]
pub enum ApplyError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("internal: {0}")]
    Internal(String),
}

#[derive(Debug)]
pub enum RebuildScope {
    Container,
    Workspace,
}

#[derive(Debug)]
pub struct PeerDependency {
    pub log: LogId,
    pub size: u64,
    pub hash_prefix: Vec<u8>,
}
