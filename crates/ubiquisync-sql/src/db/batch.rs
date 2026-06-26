use async_trait::async_trait;

use super::{DbError, DbRow, DbValue};
use crate::dialect::SqlDialect;

/// Identifies one statement queued into a [`DbBatch`].
///
/// Returned by [`DbBatch::add_statement`] and used to locate that statement's
/// [`DbStatementResult`] in the `Vec` returned by [`DbBatch::commit`]: the id
/// is the result's index, so `results[id.0]` is always this statement's
/// outcome. Holding the id means callers never have to track insertion order
/// by hand to find their own `RETURNING` rows (e.g. for emitting change
/// events after the batch commits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StmtId(pub usize);

/// The outcome of a single statement once its batch has committed.
///
/// One of these is produced per [`DbBatch::add_statement`] call, in add order.
/// `rows` carries the statement's `RETURNING` output (empty when it had no
/// `RETURNING` clause); `rows_affected` is the INSERT/UPDATE/DELETE row count,
/// which some callers need to decide whether an LWW write actually changed
/// anything.
#[derive(Debug)]
pub struct DbStatementResult {
    pub rows_affected: usize,
    pub rows: Vec<DbRow>,
}

/// An atomic, all-or-nothing unit of writes.
///
/// Statements are *collected* with [`add_statement`](DbBatch::add_statement)
/// and then run together by [`commit`](DbBatch::commit) inside a single
/// transaction. Either every statement commits or none does; on any error the
/// whole batch rolls back.
///
/// There is deliberately **no read method** on a batch. A queued statement may
/// not depend on the result of an earlier one in the same batch — all reads
/// must happen on [`Db`](super::Db) *before* the batch is assembled. This keeps
/// a batch expressible as a flat, declarative statement list, which is what
/// lets it map onto backends that have no interactive transactions (notably
/// D1's `batch()`), as well as onto real transactions (rusqlite `BEGIN/COMMIT`,
/// Durable Object `transactionSync`).
#[async_trait(?Send)]
pub trait DbBatch {
    /// The SQL dialect this batch speaks. Available here because callers build
    /// statements while holding only the batch.
    fn dialect(&self) -> SqlDialect;

    /// Queue a write statement and return its [`StmtId`]. Infallible: this only
    /// buffers `sql` and `params`; any SQL error surfaces at
    /// [`commit`](DbBatch::commit).
    fn add_statement(&mut self, sql: &str, params: &[DbValue]) -> StmtId;

    /// Commit all queued statements atomically, consuming the batch. Returns
    /// one [`DbStatementResult`] per queued statement, in add order (so a
    /// [`StmtId`] indexes straight into it). On any failure the transaction is
    /// rolled back and nothing is persisted.
    async fn commit(self: Box<Self>) -> Result<Vec<DbStatementResult>, DbError>;
}
