use ubiquisync_core::hlc::Timestamp;

use crate::db::{Db, DbBatch};

/// Translates a single op into the SQL writes that materialize it.
///
/// Split into two phases so the work maps onto every backend, including ones
/// with no interactive transaction (e.g. D1's `batch()`):
///
/// 1. [`sync_schema`](Reducer::sync_schema) runs *before* a batch is opened. It
///    is the only phase allowed to read from or issue DDL against the database.
/// 2. [`apply`](Reducer::apply) emits the op's mutation statements into an
///    already-open batch. It is pure and read-free, so the batch stays a flat,
///    declarative statement list.
pub trait Reducer {
    /// The op vocabulary this reducer materializes (e.g. the table op enum).
    type Op;
    /// The change event produced for an applied op, for downstream observers.
    type Event;
    /// Error surfaced from either phase.
    type Error;

    /// Reconcile the schema needed by `op`: create or alter tables and refresh
    /// any cached schema. Runs outside the batch — schema changes are additive
    /// and safe to commit on their own — and is the only place reads happen, so
    /// `apply` can rely on whatever this leaves cached.
    async fn sync_schema(&mut self, db: &dyn Db, op: &Self::Op) -> Result<(), Self::Error>;

    /// Emit the statements that materialize `op` at `timestamp` into `batch`.
    /// Read-free: builds SQL purely from `op` and the schema cached by
    /// [`sync_schema`](Reducer::sync_schema). The returned event is provisional
    /// until `batch` commits — the caller must drop it if the commit fails.
    async fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        op: &Self::Op,
    ) -> Result<Self::Event, Self::Error>;
}
