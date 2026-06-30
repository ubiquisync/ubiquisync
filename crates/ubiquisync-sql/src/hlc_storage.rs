//! SQL-backed [`HlcStorage`] for the hybrid logical clock.
//!
//! Owns the clock's schema — a single-row register table, named from a caller
//! prefix — and the SQL behind it. [`save`](HlcStorage::save) enqueues into a
//! [`DbBatch`] sink rather than committing on its own, so the clock state lands
//! in the same transaction as the write it timestamps. The persist upsert is
//! MAX-guarded, so an out-of-order commit can never lower the stored clock.
//!
//! The clock is read exactly once, at startup: [`SqlHlcStorage::open`] ensures
//! the table and loads the seed asynchronously, then caches it so the
//! synchronous [`load`](HlcStorage::load) the service calls just returns it.

use ubiquisync_core::hlc::HlcStorage;

use crate::{
    db::{Db, DbBatch, DbError, DbType, DbValue},
    dialect::SqlDialect,
    util::quote_ident,
};

/// Persistence for the clock register, scoped to a table-name prefix.
pub struct SqlHlcStorage {
    /// Seed read from the register at `open`, or `None` for a fresh store.
    seed: Option<u64>,
    /// Pre-rendered MAX-guarded upsert, built once from the resolved table name.
    persist_sql: String,
}

impl SqlHlcStorage {
    /// Ensure the clock register exists and read its seed — the clock's only
    /// read, done once at startup. `prefix` namespaces the table so multiple
    /// stores can share a database.
    pub async fn open(db: &dyn Db, prefix: &str) -> Result<Self, DbError> {
        let table = quote_ident(&format!("{prefix}__hlc"));
        db.exec(&create_sql(&table, db.dialect()), &[]).await?;
        let seed = match db.query(&load_sql(&table), &[]).await?.first() {
            Some(row) => Some(row.get_i64(0)? as u64),
            None => None,
        };
        Ok(Self {
            seed,
            persist_sql: persist_sql(&table, db.dialect()),
        })
    }
}

impl HlcStorage for SqlHlcStorage {
    type Error = DbError;
    type Sink = dyn DbBatch;

    /// Return the seed cached at `open`. No I/O — the read already happened.
    fn load(&self) -> Result<Option<u64>, Self::Error> {
        Ok(self.seed)
    }

    /// Enqueue the clock-state upsert into `sink`. Durable when the caller
    /// commits the sink. The packed `u64` is always below 2^48 (48-bit millis +
    /// 16-bit counter), so it round-trips through SQLite's signed 64-bit integer.
    fn save(&self, sink: &mut Self::Sink, raw: u64) -> Result<(), Self::Error> {
        sink.add_statement(&self.persist_sql, &[DbValue::Integer(raw as i64)]);
        Ok(())
    }
}

/// DDL for the register: exactly one row, pinned at `id = 1`.
fn create_sql(table: &str, dialect: SqlDialect) -> String {
    let int_type = DbType::Integer.sql_type(dialect);
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (\n    \
         id {int_type} PRIMARY KEY CHECK (id = 1),\n    \
         ts {int_type} NOT NULL DEFAULT 0\n)"
    )
}

/// Reads the single register row.
fn load_sql(table: &str) -> String {
    format!("SELECT ts FROM {table} WHERE id = 1")
}

/// MAX-guarded upsert: a stale commit cannot lower the stored clock below a
/// value an earlier-issued (but later-committed) write set.
fn persist_sql(table: &str, dialect: SqlDialect) -> String {
    let max = dialect.scalar_max();
    let p1 = dialect.placeholder(1);
    format!(
        "INSERT INTO {table} (id, ts) VALUES (1, {p1}) \
         ON CONFLICT(id) DO UPDATE SET ts = {max}(COALESCE(ts, 0), EXCLUDED.ts)"
    )
}
