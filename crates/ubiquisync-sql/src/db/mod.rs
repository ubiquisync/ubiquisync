//! The SQL backend abstraction: values, rows, batches, and the [`Db`] trait.

mod batch;
mod error;
mod schema;
mod value;

pub use batch::{DbBatch, DbStatementResult, StmtId};
pub use error::DbError;
pub use schema::{ColumnDescription, DbType, TableDescriptor};
pub use value::{DbRow, DbValue};

use async_trait::async_trait;

use crate::dialect::SqlDialect;

/// A SQL backend: reads, one-off writes/DDL, and a factory for atomic batches.
///
/// Async because backends are not all synchronous — rusqlite and Durable
/// Object `SqlStorage` are, but D1 and Postgres are Promise-/network-based.
/// Marked `?Send` (via `#[async_trait(?Send)]`) so the same trait compiles on
/// `wasm32` targets (Cloudflare Workers), where the underlying JS futures are
/// not `Send`. Native multi-threaded backends are still free to be `Send`
/// concretely; we just don't *require* it at the trait boundary.
#[async_trait(?Send)]
pub trait Db {
    /// The SQL dialect this backend speaks (placeholder syntax, upsert verbs,
    /// type names). Synchronous: it's pure metadata, no I/O.
    fn dialect(&self) -> SqlDialect;

    /// Introspect a table's columns and primary key, or `None` if it does not
    /// exist. Used for schema reconciliation *before* a batch is built.
    async fn describe_table(&self, name: &str) -> Result<Option<TableDescriptor>, DbError>;

    /// Execute a single statement outside any batch (autocommit). For DDL
    /// (`CREATE TABLE`, `ALTER TABLE ... ADD COLUMN`) and one-off writes.
    /// Returns the number of rows affected.
    async fn exec(&self, sql: &str, params: &[DbValue]) -> Result<usize, DbError>;

    /// Run a read query and return every row. Materializes the full result
    /// set; not for unbounded scans.
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError>;

    /// Open a fresh, empty batch. Allocation only — no transaction is started
    /// and nothing touches the backend until [`DbBatch::commit`].
    fn new_batch(&self) -> Box<dyn DbBatch>;
}
