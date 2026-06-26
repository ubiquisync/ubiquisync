use crate::col_type::ColType;
use crate::dialect::SqlDialect;
use async_trait::async_trait;

/// A Db value — used for both parameters and query results.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
    Uuid([u8; 16]),
}

#[derive(Debug)]
pub struct DbRow {
    pub values: Vec<DbValue>,
}

impl DbRow {
    pub fn get_i64(&self, idx: usize) -> Result<i64, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(*v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_text(&self, idx: usize) -> Result<&str, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "text",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_blob(&self, idx: usize) -> Result<&[u8], DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(v),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_bool(&self, idx: usize) -> Result<bool, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(*v != 0),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "bool/integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_i64(&self, idx: usize) -> Result<Option<i64>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Integer(v)) => Ok(Some(*v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "integer",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_text(&self, idx: usize) -> Result<Option<&str>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Text(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "text",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_uuid(&self, idx: usize) -> Result<[u8; 16], DbError> {
        // TODO match uuid or bytes
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                col: idx,
                expected: "16-byte UUID blob",
            }),
            Some(DbValue::Null) => Err(DbError::UnexpectedNull(idx)),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "16-byte UUID blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_uuid(&self, idx: usize) -> Result<Option<[u8; 16]>, DbError> {
        // TODO match uuid or bytes
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => {
                let arr = v.as_slice().try_into().map_err(|_| DbError::TypeMismatch {
                    col: idx,
                    expected: "16-byte UUID blob",
                })?;
                Ok(Some(arr))
            }
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "16-byte UUID blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }

    pub fn get_optional_blob(&self, idx: usize) -> Result<Option<&[u8]>, DbError> {
        match self.values.get(idx) {
            Some(DbValue::Blob(v)) => Ok(Some(v)),
            Some(DbValue::Null) => Ok(None),
            Some(_) => Err(DbError::TypeMismatch {
                col: idx,
                expected: "blob",
            }),
            None => Err(DbError::ColumnOutOfBounds(idx)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sql error: {0}")]
    Sql(String),
    #[error("type mismatch at column {col}: expected {expected}")]
    TypeMismatch { col: usize, expected: &'static str },
    #[error("column index {0} out of bounds")]
    ColumnOutOfBounds(usize),
    #[error("unexpected null at column {0}")]
    UnexpectedNull(usize),
}

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
    fn dialect(&self) -> &dyn SqlDialect;

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

/// An atomic, all-or-nothing unit of writes.
///
/// Statements are *collected* with [`add_statement`](DbBatch::add_statement)
/// and then run together by [`commit`](DbBatch::commit) inside a single
/// transaction. Either every statement commits or none does; on any error the
/// whole batch rolls back.
///
/// There is deliberately **no read method** on a batch. A queued statement may
/// not depend on the result of an earlier one in the same batch — all reads
/// must happen on [`Db`] *before* the batch is assembled. This keeps a batch
/// expressible as a flat, declarative statement list, which is what lets it map
/// onto backends that have no interactive transactions (notably D1's
/// `batch()`), as well as onto real transactions (rusqlite `BEGIN/COMMIT`,
/// Durable Object `transactionSync`).
#[async_trait(?Send)]
pub trait DbBatch {
    /// The SQL dialect this batch speaks. Available here because callers build
    /// statements while holding only the batch.
    fn dialect(&self) -> &dyn SqlDialect;

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

pub struct TableDescriptor {
    pub name: String,
    pub pk_cols: Vec<ColumnDescription>,
    pub cols: Vec<ColumnDescription>,
}

pub struct ColumnDescription {
    pub name: String,
    pub db_type: DbType,
    pub nullable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DbType {
    Integer,
    Text,
    Blob,
    Uuid,
}

impl DbType {
    pub fn is_valid_for(self, col_type: ColType) -> bool {
        match col_type {
            ColType::Bytes => self == DbType::Blob,
            ColType::Text => self == DbType::Text,
            ColType::I64 => self == DbType::Integer,
            ColType::Uuid => self == DbType::Uuid || self == DbType::Blob,
            ColType::MaxI64 => self == DbType::Integer,
        }
    }
}
