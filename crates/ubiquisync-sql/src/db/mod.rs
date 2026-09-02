//! The SQL backend abstraction: values, rows, batches, and the [`Db`] trait.

mod batch;
mod col;
mod error;
mod macros;
mod row;
mod schema;
pub mod sea_query;
mod value;

pub use batch::*;
pub use col::*;
pub use error::*;
pub use row::*;
pub use schema::*;
pub use value::*;

use async_trait::async_trait;

use crate::dialect::SqlDialect;

/// A SQL backend: reads, one-off writes/DDL, and a factory for atomic batches.
//
// TODO(wasm): a Cloudflare D1 / Durable-Objects backend has `!Send` JS-Promise
// futures; when it lands it must satisfy this `Send + Sync` bound internally
// (wrap its handle/futures in `send_wrapper::SendWrapper`, sound on the
// single-threaded Workers isolate) rather than relaxing the trait for every
// target. Alternatively we can make the Send + Sync requirement conditional on
// the build target. Deferred until that backend is built.
#[async_trait]
pub trait Db: Send + Sync {
    /// The SQL dialect this backend speaks (placeholder syntax, upsert verbs,
    /// type names). Synchronous: it's pure metadata, no I/O.
    fn dialect(&self) -> SqlDialect;

    /// Introspect a table's columns and primary key, or `None` if it does not
    /// exist. Used for schema reconciliation *before* a batch is built.
    async fn describe_table(&self, name: &str) -> Result<Option<DbTableDescriptor>, DbError>;

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
