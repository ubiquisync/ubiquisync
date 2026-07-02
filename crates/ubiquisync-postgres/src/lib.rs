//! Postgres storage backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate implements the [`Db`] backend abstraction over
//! [`tokio_postgres`].
//!
//! # Single writer, many readers
//!
//! [`PostgresDb`] splits its work across two connection sets:
//!
//! * **Writer** — one connection ([`exec`](Db::exec), batches, and schema
//!   introspection). Writes are expected to be serialized by the caller (the
//!   reducer layer), so the writer's lock is uncontended; keeping writes on
//!   their own connection also means an open `BEGIN`/`COMMIT` can never capture
//!   an unrelated statement.
//! * **Reader** — a pool of read-only connections ([`query`](Db::query)). Reads
//!   run concurrently on their own connections and are never blocked by the
//!   write lock; MVCC still gives them a consistent committed snapshot even
//!   while a write transaction is open. Every reader connection is forced
//!   read-only at the session level, so an accidental write there is rejected
//!   rather than silently applied.

use std::error::Error as StdError;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use deadpool_postgres::{Manager, ManagerConfig, Pool, PoolError, RecyclingMethod};
use futures_util::{TryStreamExt, pin_mut};
use tokio::sync::Mutex;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{IsNull, ToSql, Type, to_sql_checked};
use tokio_postgres::{Client, Config, NoTls, Row, RowStream};
use uuid::Uuid;

use ubiquisync_sql::db::{
    Db, DbBatch, DbColumnDescription, DbError, DbRow, DbStatementResult, DbTableDescriptor, DbType,
    DbValue, StmtId,
};
use ubiquisync_sql::dialect::SqlDialect;

/// A [`Db`] backed by a single-writer connection and a pool of read-only
/// readers.
///
/// The `writer` is an [`Arc`]`<`[`Mutex`]`<`[`Client`]`>>`: batches handed out
/// by [`new_batch`](Db::new_batch) run against it, and it opens a real
/// `BEGIN`/`COMMIT` (via [`Client::transaction`], which needs `&mut`). The lock
/// is what lets a batch hold the writer exclusively for its transaction; it is
/// uncontended when the caller serializes writes, and the read path never takes
/// it. The `reader` is a [`Pool`] of connections forced read-only at the session
/// level.
#[derive(Clone)]
pub struct PostgresDb {
    writer: Arc<Mutex<Client>>,
    reader: Pool,
}

impl PostgresDb {
    /// Connect with separate write and read endpoints, over unencrypted
    /// connections.
    ///
    /// `write_url` backs the single writer; `read_url` backs a [`Pool`] of
    /// `read_pool_size` reader connections. It should be a read-only *role* on
    /// the same database as `write_url` — not a separate read replica, since
    /// startup reads back rows it just wrote, which a lagging replica could miss.
    /// Reader connections are *additionally* forced read-only at the session
    /// level, so a write attempted on one is rejected (SQLSTATE `25006`)
    /// regardless of the role's grants.
    ///
    /// TLS is not wired up yet; both endpoints use `NoTls`.
    pub async fn connect(
        write_url: &str,
        read_url: &str,
        read_pool_size: usize,
    ) -> Result<Self, DbError> {
        let writer = connect_writer(write_url).await?;
        let reader = build_read_pool(read_url, read_pool_size)?;
        Ok(Self { writer, reader })
    }

    /// Connect using one endpoint for both the writer and the reader pool — a
    /// convenience over [`connect`](Self::connect) for a single database with no
    /// separate read-only role or replica. The reader pool is still forced
    /// read-only at the session level.
    pub async fn connect_single(url: &str, read_pool_size: usize) -> Result<Self, DbError> {
        Self::connect(url, url, read_pool_size).await
    }
}

/// Open the single writer connection and spawn the task that drives its socket.
///
/// The task runs until the connection closes; if it ends early, subsequent
/// client calls fail with a clear error rather than hanging.
async fn connect_writer(url: &str) -> Result<Arc<Mutex<Client>>, DbError> {
    let (client, connection) = tokio_postgres::connect(url, NoTls).await.map_err(map_err)?;
    // The connection future must be polled for the client to make progress; its
    // own errors surface on the client side, so we just let it end.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(Arc::new(Mutex::new(client)))
}

/// Build the reader [`Pool`], forcing every connection read-only.
///
/// `default_transaction_read_only=on` is set as a *startup* option (`-c …`), not
/// a runtime `SET`, so it is applied the moment each connection is born and
/// survives pool recycling. [`RecyclingMethod::Fast`] avoids a `RESET` that
/// would otherwise clear it.
fn build_read_pool(url: &str, size: usize) -> Result<Pool, DbError> {
    let mut config: Config = url.parse().map_err(|e: tokio_postgres::Error| map_err(e))?;
    // Force read-only via a startup option. `Config::options` replaces rather
    // than appends, so preserve any options the caller already set in the URL
    // (e.g. `statement_timeout`, `application_name`); multiple options are
    // space-separated in the one string. The read-only flag goes first so it is
    // authoritative.
    const READ_ONLY: &str = "-c default_transaction_read_only=on";
    let options = match config.get_options() {
        Some(existing) => format!("{READ_ONLY} {existing}"),
        None => READ_ONLY.to_string(),
    };
    config.options(options);

    let manager = Manager::from_config(
        config,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Pool::builder(manager)
        .max_size(size)
        .build()
        .map_err(|e| DbError::Sql(format!("failed to build read pool: {e}")))
}

#[async_trait(?Send)]
impl Db for PostgresDb {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    async fn describe_table(&self, name: &str) -> Result<Option<DbTableDescriptor>, DbError> {
        // Introspect via `pg_catalog` rather than `information_schema`: it exposes
        // the primary-key column order (`pg_constraint.conkey`) directly, which
        // we need to report composite keys in declared order. `pg_table_is_visible`
        // resolves the table through the session's `search_path`, matching how
        // unqualified names in our DDL/DML resolve. `array_position` yields the
        // 1-based key position for PK columns and NULL (→ 0) for the rest.
        const SQL: &str = "\
            SELECT a.attname, t.typname, a.attnotnull, \
                   COALESCE(array_position(pk.conkey, a.attnum), 0) AS pk_pos \
            FROM pg_attribute a \
            JOIN pg_class c ON c.oid = a.attrelid \
            JOIN pg_type t ON t.oid = a.atttypid \
            LEFT JOIN pg_constraint pk ON pk.conrelid = c.oid AND pk.contype = 'p' \
            WHERE c.relname = $1 AND pg_table_is_visible(c.oid) \
              AND a.attnum > 0 AND NOT a.attisdropped \
            ORDER BY a.attnum";

        // Introspection runs on the writer, not a reader: it gates schema
        // reconciliation immediately before DDL, so it must see the writer's own
        // just-committed state — a lagging read replica could miss a table the
        // writer just created.
        let client = self.writer.lock().await;
        let rows = client.query(SQL, &[&name]).await.map_err(map_err)?;

        let mut pk: Vec<(i32, DbColumnDescription)> = Vec::new();
        let mut cols: Vec<DbColumnDescription> = Vec::new();
        for row in &rows {
            // `attname`/`typname` are the `name` type, which `String` accepts.
            let col_name: String = row.try_get(0).map_err(map_err)?;
            let type_name: String = row.try_get(1).map_err(map_err)?;
            let not_null: bool = row.try_get(2).map_err(map_err)?;
            let pk_pos: i32 = row.try_get(3).map_err(map_err)?;
            let desc = DbColumnDescription {
                name: col_name,
                db_type: db_type_from_pg(&type_name),
                nullable: !not_null,
            };
            if pk_pos > 0 {
                pk.push((pk_pos, desc));
            } else {
                cols.push(desc);
            }
        }

        if pk.is_empty() && cols.is_empty() {
            return Ok(None);
        }

        // Order primary-key columns by their declared key position.
        pk.sort_by_key(|(pos, _)| *pos);
        let pk_cols = pk.into_iter().map(|(_, desc)| desc).collect();

        Ok(Some(DbTableDescriptor {
            name: name.to_string(),
            pk_cols,
            cols,
        }))
    }

    async fn exec(&self, sql: &str, params: &[DbValue]) -> Result<usize, DbError> {
        let client = self.writer.lock().await;
        let bound = bind(params);
        let stream = client.query_raw(sql, refs(&bound)).await.map_err(map_err)?;
        let (_, rows_affected) = drain(stream).await?;
        Ok(rows_affected)
    }

    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        // Reads run on their own pooled connection, so they proceed concurrently
        // and never wait on the writer's lock.
        let client = self.reader.get().await.map_err(map_pool_err)?;
        let bound = bind(params);
        let stream = client.query_raw(sql, refs(&bound)).await.map_err(map_err)?;
        let (rows, _) = drain(stream).await?;
        Ok(rows)
    }

    fn new_batch(&self) -> Box<dyn DbBatch> {
        Box::new(PostgresBatch {
            writer: Arc::clone(&self.writer),
            statements: Vec::new(),
        })
    }
}

/// An atomic batch of writes against a [`PostgresDb`], run on the writer.
struct PostgresBatch {
    writer: Arc<Mutex<Client>>,
    statements: Vec<(String, Vec<DbValue>)>,
}

#[async_trait(?Send)]
impl DbBatch for PostgresBatch {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }

    fn add_statement(&mut self, sql: &str, params: &[DbValue]) -> StmtId {
        let id = StmtId(self.statements.len());
        self.statements.push((sql.to_string(), params.to_vec()));
        id
    }

    async fn commit(self: Box<Self>) -> Result<Vec<DbStatementResult>, DbError> {
        // Hold the writer exclusively for the whole transaction so no other
        // `exec`/batch can interleave a statement into it. (Reads live on the
        // reader pool and are unaffected.)
        let mut client = self.writer.lock().await;

        // A real interactive transaction: Postgres compute is colocated with the
        // data. On any early return the `Transaction` drops and rolls back — the
        // first failed statement already aborts the transaction server-side, so
        // nothing is persisted.
        let txn = client.transaction().await.map_err(map_err)?;
        let mut results = Vec::with_capacity(self.statements.len());
        for (sql, params) in &self.statements {
            let bound = bind(params);
            let stream = txn
                .query_raw(sql.as_str(), refs(&bound))
                .await
                .map_err(map_err)?;
            let (rows, rows_affected) = drain(stream).await?;
            results.push(DbStatementResult {
                rows_affected,
                rows,
            });
        }
        txn.commit().await.map_err(map_err)?;
        Ok(results)
    }
}

/// Drain a [`RowStream`] to completion, returning its rows and affected-row
/// count.
///
/// The stream must be fully consumed before [`RowStream::rows_affected`] reports
/// anything — it returns `None` until then — so the count is read only after the
/// loop exhausts it. That count is the command tag's row total: for a
/// conditional `INSERT … ON CONFLICT DO UPDATE … WHERE`, it is 0 when the guard
/// holds the write off and 1 when it applies, which is the signal a conditional
/// LWW upsert needs (the analogue of SQLite's `Connection::changes()`).
async fn drain(stream: RowStream) -> Result<(Vec<DbRow>, usize), DbError> {
    pin_mut!(stream);
    let mut rows = Vec::new();
    while let Some(row) = stream.try_next().await.map_err(map_err)? {
        rows.push(map_row(&row)?);
    }
    // Exhausted above, so `rows_affected` is populated; a statement that reports
    // none (e.g. DDL) has no meaningful count and reads as 0.
    let rows_affected = stream.rows_affected().unwrap_or(0) as usize;
    Ok((rows, rows_affected))
}

/// Map a [`tokio_postgres`] result row to a [`DbRow`], one column at a time.
fn map_row(row: &Row) -> Result<DbRow, DbError> {
    let mut values = Vec::with_capacity(row.len());
    for i in 0..row.len() {
        values.push(map_value(row, i)?);
    }
    Ok(DbRow { values })
}

/// Map result column `idx` to a [`DbValue`], dispatching on the column's Postgres
/// type.
///
/// Only the classes the engine emits are recognized — the signed integers,
/// text, `BYTEA`, and native `UUID` — plus `BOOL` (surfaced as an integer, since
/// [`DbValue`] has no bool). Each is read as an `Option` so SQL NULL maps to
/// [`DbValue::Null`]. Any other type is a hard error rather than a silent
/// coercion: it means a query returned a column outside the engine's vocabulary,
/// which the caller should see, not paper over.
fn map_value(row: &Row, idx: usize) -> Result<DbValue, DbError> {
    let ty = row.columns()[idx].type_();
    let value = if *ty == Type::INT8 {
        opt(row.try_get::<_, Option<i64>>(idx).map_err(map_err)?, |v| {
            DbValue::Integer(v)
        })
    } else if *ty == Type::INT4 {
        opt(row.try_get::<_, Option<i32>>(idx).map_err(map_err)?, |v| {
            DbValue::Integer(v as i64)
        })
    } else if *ty == Type::INT2 {
        opt(row.try_get::<_, Option<i16>>(idx).map_err(map_err)?, |v| {
            DbValue::Integer(v as i64)
        })
    } else if *ty == Type::BOOL {
        opt(row.try_get::<_, Option<bool>>(idx).map_err(map_err)?, |v| {
            DbValue::Integer(v as i64)
        })
    } else if *ty == Type::TEXT || *ty == Type::VARCHAR || *ty == Type::BPCHAR || *ty == Type::NAME
    {
        opt(
            row.try_get::<_, Option<String>>(idx).map_err(map_err)?,
            DbValue::Text,
        )
    } else if *ty == Type::BYTEA {
        opt(
            row.try_get::<_, Option<Vec<u8>>>(idx).map_err(map_err)?,
            DbValue::Blob,
        )
    } else if *ty == Type::UUID {
        opt(row.try_get::<_, Option<Uuid>>(idx).map_err(map_err)?, |v| {
            DbValue::Uuid(*v.as_bytes())
        })
    } else {
        return Err(DbError::Sql(format!(
            "unsupported postgres type `{ty}` in result column {idx}"
        )));
    };
    Ok(value)
}

/// Map an `Option<T>` result cell to a [`DbValue`]: NULL → [`DbValue::Null`],
/// otherwise `f(value)`.
fn opt<T>(value: Option<T>, f: impl FnOnce(T) -> DbValue) -> DbValue {
    value.map_or(DbValue::Null, f)
}

/// Map a Postgres `typname` to a generic [`DbType`], recognizing only the types
/// the engine emits as DDL (see [`DbType::sql_type`]).
///
/// `int8` (`BIGINT`) is the only integer we write; `text`, `bytea`, and native
/// `uuid` cover the rest. Anything else — a column added out of band, an enum, a
/// numeric — maps to [`DbType::Other`], an honest "outside the engine's
/// vocabulary" that schema reconciliation treats as a mismatch rather than
/// coercing to a class it isn't.
///
/// Deliberately narrower than [`map_value`]: introspection only ever sees
/// engine-emitted DDL columns (exactly the four types above), whereas a query
/// result can surface `int4`/`bool`/etc. from expressions, which `map_value`
/// must therefore also handle. Do not widen this to match `map_value` — an
/// `int4` *column* really is outside the engine's vocabulary and should reconcile
/// as a mismatch, not be silently accepted as an integer.
fn db_type_from_pg(type_name: &str) -> DbType {
    match type_name {
        "int8" => DbType::Integer,
        "text" => DbType::Text,
        "bytea" => DbType::Blob,
        "uuid" => DbType::Uuid,
        _ => DbType::Other,
    }
}

/// A bound parameter: a [`DbValue`] borrowed for the length of one statement.
///
/// A local newtype is required because [`ToSql`] and [`DbValue`] are both
/// foreign to this crate, so [`DbValue`] cannot implement [`ToSql`] directly.
#[derive(Debug)]
struct Param<'a>(&'a DbValue);

impl ToSql for Param<'_> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        match self.0 {
            // An untyped NULL: `IsNull::Yes` binds SQL NULL for *any* column
            // type. A typed `Option::<T>::None` would instead reject every column
            // whose type isn't `T`, so we can't bind NULL that way without knowing
            // each column's type up front.
            DbValue::Null => Ok(IsNull::Yes),
            DbValue::Integer(i) => i.to_sql(ty, out),
            DbValue::Text(s) => s.to_sql(ty, out),
            // `Vec<u8>` encodes as `BYTEA`.
            DbValue::Blob(b) => b.to_sql(ty, out),
            // Bind the 16 raw bytes as the native `UUID` type.
            DbValue::Uuid(u) => Uuid::from_bytes(*u).to_sql(ty, out),
        }
    }

    // Accept any column type: a real NULL fits anywhere, and for non-null values
    // we delegate to the inner encoder, letting the server reject a genuine
    // mismatch rather than pre-rejecting here. This trades the driver's
    // client-side type check for a server-side one, so correctness relies on the
    // caller binding a `DbValue` variant matching the column — which the engine
    // always does (each placeholder maps to a column of the corresponding type).
    fn accepts(_ty: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// Wrap each [`DbValue`] in a [`Param`] for binding. The returned `Vec` owns the
/// wrappers and must outlive the `query_raw` call; [`refs`] borrows from it to
/// produce the trait-object slice the driver wants.
fn bind(params: &[DbValue]) -> Vec<Param<'_>> {
    params.iter().map(Param).collect()
}

/// View the bound [`Param`]s as `&(dyn ToSql + Sync)` trait objects.
///
/// [`Client::query_raw`] takes an `ExactSizeIterator` of `BorrowToSql`, which
/// `&(dyn ToSql + Sync)` implements. Kept separate from [`bind`] so the owned
/// `Vec<Param>` stays live on the caller's stack across the `.await`.
fn refs<'a>(bound: &'a [Param<'a>]) -> Vec<&'a (dyn ToSql + Sync)> {
    bound.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}

/// Translate a [`tokio_postgres`] error into a [`DbError`], mapping a unique /
/// primary-key violation (SQLSTATE `23505`) to [`DbError::UniqueViolation`] so
/// op-log ingestion can detect an already-ingested entry.
///
/// For any other server-side error, `tokio_postgres`'s own `Display` is just the
/// terse category (`"db error"`); the useful SQLSTATE and message live on the
/// [`DbError`](tokio_postgres::error::DbError) payload, so we splice them in.
/// Client-side errors (parameter encoding, row decoding) have a descriptive
/// `Display` and pass through as-is.
fn map_err(err: tokio_postgres::Error) -> DbError {
    if let Some(db_err) = err.as_db_error() {
        if db_err.code() == &SqlState::UNIQUE_VIOLATION {
            return DbError::UniqueViolation;
        }
        return DbError::Sql(format!("{}: {}", db_err.code().code(), db_err.message()));
    }
    DbError::Sql(err.to_string())
}

/// Translate a reader-[`Pool`] checkout failure into a [`DbError`]. A pool error
/// wrapping a backend error still carries the underlying detail via its
/// `Display`.
fn map_pool_err(err: PoolError) -> DbError {
    DbError::Sql(format!("read pool error: {err}"))
}
