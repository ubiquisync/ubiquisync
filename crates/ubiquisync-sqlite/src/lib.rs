//! SQLite storage backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate implements the [`Db`] backend abstraction
//! over [`rusqlite`], reporting
//! [`SqlDialect::Sqlite`]. The SQL
//! flavor itself is not implemented here — it lives in `ubiquisync-sql`; this
//! crate only drives the connection.
//!
//! # Single writer, one reader
//!
//! [`SqliteDb`] splits its work across two connections to the same database:
//!
//! * **Writer** — one connection ([`exec`](Db::exec), batches, and schema
//!   introspection), opened in WAL mode. SQLite has a single write slot anyway,
//!   so serializing writes behind its mutex matches the engine.
//! * **Reader** — one connection ([`query`](Db::query)) pinned read-only with
//!   `PRAGMA query_only`, so a write that reaches the read path is rejected by
//!   the engine (`SQLITE_READONLY`) rather than silently applied. On a file
//!   database (WAL) its reads also see a consistent committed snapshot without
//!   blocking, or being blocked by, the writer — the isolation Postgres gets from
//!   MVCC. In-memory databases have no WAL, so there the reader still gets
//!   read-only enforcement but not snapshot isolation.

use std::path::Path;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, params_from_iter};

use ubiquisync_sql::db::{
    Db, DbBatch, DbColumnDescription, DbError, DbRow, DbStatementResult, DbTableDescriptor, DbType,
    DbValue, StmtId,
};
use ubiquisync_sql::dialect::SqlDialect;

/// A [`Db`] backed by a writer connection and a read-only reader connection to
/// the same database. See the [module docs](crate) for the split.
///
/// Both connections are wrapped in an [`Arc<Mutex<_>>`]. The `writer` takes
/// [`exec`](Db::exec), batches handed out by [`new_batch`](Db::new_batch), and
/// schema introspection; its mutex serializes writes to match SQLite's single
/// write slot. The `reader` takes [`query`](Db::query) and is pinned read-only
/// (`PRAGMA query_only`), so a stray write on the read path is rejected rather
/// than silently applied.
#[derive(Clone)]
pub struct SqliteDb {
    writer: Arc<Mutex<Connection>>,
    reader: Arc<Mutex<Connection>>,
}

/// Distinguishes the shared in-memory databases handed out by
/// [`SqliteDb::open_in_memory`], so each call gets its own private database that
/// its writer and reader still share.
#[cfg(any(test, feature = "test-support"))]
static MEM_DB_SEQ: AtomicU64 = AtomicU64::new(0);

impl SqliteDb {
    /// Open (creating if needed) a database file at `path`, in WAL mode, with a
    /// separate read-only reader connection.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let path = path.as_ref();
        let writer = Connection::open(path).map_err(map_err)?;
        configure_writer(&writer)?;
        // The reader opens the same file read-write at the OS level but is pinned
        // `query_only`. A plain `SQLITE_OPEN_READ_ONLY` handle can't touch the
        // `-wal`/`-shm` files a WAL reader needs, whereas `query_only` rejects SQL
        // writes while still participating in WAL.
        let reader = Connection::open(path).map_err(map_err)?;
        configure_reader(&reader)?;
        Ok(Self::from_parts(writer, reader))
    }

    /// Open a fresh, private in-memory database with the same writer/reader
    /// split. **Test-only** — gated behind the `test-support` feature.
    ///
    /// This is pointless outside tests: an in-memory database can't use WAL, so
    /// the reader gets read-only enforcement but *not* the snapshot isolation
    /// that motivates the split for a real file database. It exists only so the
    /// suites can run without touching the filesystem.
    ///
    /// The database is a uniquely-named shared-cache in-memory database, so the
    /// writer and reader connections address the *same* data — a bare `:memory:`
    /// is private per connection, so the reader would see an empty database. It
    /// lives as long as this [`SqliteDb`] keeps either connection open.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let seq = MEM_DB_SEQ.fetch_add(1, Ordering::Relaxed);
        // `Connection::open` parses `file:` URIs because rusqlite enables
        // `SQLITE_OPEN_URI` by default; `cache=shared` is what lets the two
        // connections attach to the same in-memory database.
        let uri = format!("file:ubq-mem-{seq}?mode=memory&cache=shared");
        let writer = Connection::open(&uri).map_err(map_err)?;
        configure_writer(&writer)?;
        let reader = Connection::open(&uri).map_err(map_err)?;
        configure_reader(&reader)?;
        Ok(Self::from_parts(writer, reader))
    }

    /// Wrap an already-open writer/reader connection pair.
    fn from_parts(writer: Connection, reader: Connection) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
            reader: Arc::new(Mutex::new(reader)),
        }
    }
}

/// Acquire a connection, mapping a poisoned mutex to a [`DbError`] rather than
/// panicking. A poison means a prior caller panicked mid-statement, so the
/// connection may be mid-transaction; we surface that as an error and let the
/// caller decide, instead of crashing the whole process.
fn lock(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, DbError> {
    conn.lock()
        .map_err(|_| DbError::Sql("sqlite connection mutex poisoned".to_string()))
}

/// Put the writer connection in WAL mode and give it a short busy timeout.
///
/// WAL is a persistent, file-level property set once at open; it is what makes
/// the reader's snapshot reads non-blocking for the life of a file database. On
/// an in-memory database `journal_mode=WAL` is silently a no-op (memory
/// databases keep their in-memory journal). The busy timeout smooths over
/// transient `SQLITE_BUSY` from checkpoint contention on files.
fn configure_writer(conn: &Connection) -> Result<(), DbError> {
    // `journal_mode` reports the resulting mode as a row, so read it rather than
    // using `pragma_update` (which is for pragmas that return nothing).
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
        .map_err(map_err)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(map_err)?;
    Ok(())
}

/// Pin the reader connection read-only at the session level.
///
/// `query_only` makes the engine reject any INSERT/UPDATE/DELETE/DDL with
/// `SQLITE_READONLY`, so a write mistakenly routed through [`Db::query`] fails
/// loudly instead of landing. It is the SQLite analogue of Postgres's
/// `default_transaction_read_only`, and unlike opening the file
/// `SQLITE_OPEN_READ_ONLY` it stays compatible with WAL.
fn configure_reader(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "query_only", true).map_err(map_err)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(map_err)?;
    Ok(())
}

#[async_trait(?Send)]
impl Db for SqliteDb {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    async fn describe_table(&self, name: &str) -> Result<Option<DbTableDescriptor>, DbError> {
        // Introspection runs on the writer, not the reader: it gates schema
        // reconciliation immediately before DDL, so it must see the writer's own
        // just-committed state.
        let conn = lock(&self.writer)?;

        // `pragma_table_info` is a table-valued function, so the table name can
        // be bound as a parameter (a bare `PRAGMA table_info(...)` cannot). It
        // yields no rows for a table that does not exist.
        let mut stmt = conn
            .prepare(r#"SELECT name, type, "notnull", pk FROM pragma_table_info(?1)"#)
            .map_err(map_err)?;

        // (name, db_type, nullable, pk_position): pk is 0 for non-key columns
        // and the 1-based position within the primary key otherwise.
        let rows = stmt
            .query_map([name], |row| {
                let col_name: String = row.get(0)?;
                let type_name: String = row.get(1)?;
                let notnull: i64 = row.get(2)?;
                let pk: i64 = row.get(3)?;
                Ok((col_name, affinity(&type_name), notnull == 0, pk))
            })
            .map_err(map_err)?;

        let mut pk: Vec<(i64, DbColumnDescription)> = Vec::new();
        let mut cols: Vec<DbColumnDescription> = Vec::new();
        for row in rows {
            let (col_name, db_type, nullable, pk_pos) = row.map_err(map_err)?;
            let desc = DbColumnDescription {
                name: col_name,
                db_type,
                nullable,
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
        let conn = lock(&self.writer)?;
        let result = run_statement(&conn, sql, params)?;
        Ok(result.rows_affected)
    }

    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        // Reads run on the read-only reader connection, so a write here is
        // rejected by `query_only` and, on a file (WAL), reads never wait on the
        // writer's lock.
        let conn = lock(&self.reader)?;
        let result = run_statement(&conn, sql, params)?;
        Ok(result.rows)
    }

    fn new_batch(&self) -> Box<dyn DbBatch> {
        Box::new(SqliteBatch {
            writer: Arc::clone(&self.writer),
            statements: Vec::new(),
        })
    }
}

/// An atomic batch of writes against a [`SqliteDb`], run on the writer.
struct SqliteBatch {
    writer: Arc<Mutex<Connection>>,
    statements: Vec<(String, Vec<DbValue>)>,
}

#[async_trait(?Send)]
impl DbBatch for SqliteBatch {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    fn add_statement(&mut self, sql: &str, params: &[DbValue]) -> StmtId {
        let id = StmtId(self.statements.len());
        self.statements.push((sql.to_string(), params.to_vec()));
        id
    }

    async fn commit(self: Box<Self>) -> Result<Vec<DbStatementResult>, DbError> {
        let conn = lock(&self.writer)?;

        // A real interactive transaction: compute is colocated with the data,
        // so we open BEGIN/COMMIT directly. Any error returns early and drops
        // the transaction, which rolls back — nothing is persisted.
        let txn = conn.unchecked_transaction().map_err(map_err)?;
        let mut results = Vec::with_capacity(self.statements.len());
        for (sql, params) in &self.statements {
            results.push(run_statement(&txn, sql, params)?);
        }
        txn.commit().map_err(map_err)?;
        Ok(results)
    }
}

/// Prepare, bind, and run a single statement, draining any `RETURNING` rows.
///
/// Works for DDL, writes, reads, and `RETURNING` writes uniformly: we always go
/// through `prepare`/`query` (rather than `execute`, which errors on a statement
/// that yields rows).
///
/// `rows_affected` is `Connection::changes()`: the direct row count of the most
/// recent INSERT/UPDATE/DELETE, excluding trigger/cascade rows. This is the
/// signal a conditional LWW upsert needs — 1 when the target row was written
/// (inserted, or conflict-updated with its `WHERE` passing) and 0 when the
/// `WHERE` held the write off — and it stays correct even when DDL precedes the
/// upsert, because the upsert is itself DML and resets the counter to its own
/// result.
///
/// **Known failure mode:** `changes()` is *not reset* by statements that aren't
/// INSERT/UPDATE/DELETE. So `exec`-ing a DDL or SELECT statement reports the row
/// count of the *previous* write on this connection, not 0. We accept this: no
/// caller reads a DDL's or SELECT's `rows_affected` (a DDL row count is
/// meaningless, and reads go through [`Db::query`], which discards it), and
/// every statement whose count *is* consulted — the upserts and other DML —
/// resets `changes()` first, so the consulted value is always its own.
fn run_statement(
    conn: &Connection,
    sql: &str,
    params: &[DbValue],
) -> Result<DbStatementResult, DbError> {
    let mut stmt = conn.prepare(sql).map_err(map_err)?;
    let col_count = stmt.column_count();

    let mut rows_out = Vec::new();
    let mut rows = stmt
        .query(params_from_iter(params.iter().map(to_sql_value)))
        .map_err(map_err)?;
    while let Some(row) = rows.next().map_err(map_err)? {
        let mut values = Vec::with_capacity(col_count);
        for i in 0..col_count {
            values.push(value_from_ref(row.get_ref(i).map_err(map_err)?)?);
        }
        rows_out.push(DbRow { values });
    }
    drop(rows);

    Ok(DbStatementResult {
        rows_affected: conn.changes() as usize,
        rows: rows_out,
    })
}

/// Bind a [`DbValue`] parameter, borrowing its bytes rather than copying.
///
/// Returns a [`ToSqlOutput::Borrowed`] [`ValueRef`] that points straight into the
/// `DbValue`, so binding a large text/blob (or a tight write loop) costs no
/// per-statement allocation — the buffer is read in place at bind time. UUIDs
/// are bound as their 16 raw bytes, matching [`DbType::Uuid`]'s `BLOB` storage
/// class.
fn to_sql_value(value: &DbValue) -> ToSqlOutput<'_> {
    let value_ref = match value {
        DbValue::Null => ValueRef::Null,
        DbValue::Integer(i) => ValueRef::Integer(*i),
        DbValue::Text(s) => ValueRef::Text(s.as_bytes()),
        DbValue::Blob(b) => ValueRef::Blob(b.as_slice()),
        DbValue::Uuid(u) => ValueRef::Blob(u.as_slice()),
    };
    ToSqlOutput::Borrowed(value_ref)
}

/// Map a rusqlite result cell to a [`DbValue`].
///
/// SQLite never reports `Uuid`; 16-byte UUIDs come back as `Blob` and the row
/// accessors (`get_uuid`) reinterpret them. `Real` has no [`DbValue`] analogue —
/// the engine never stores floats — so it is a type error if one appears.
fn value_from_ref(value: ValueRef<'_>) -> Result<DbValue, DbError> {
    Ok(match value {
        ValueRef::Null => DbValue::Null,
        ValueRef::Integer(i) => DbValue::Integer(i),
        ValueRef::Text(bytes) => DbValue::Text(
            std::str::from_utf8(bytes)
                .map_err(|e| DbError::Sql(format!("invalid utf-8 in text column: {e}")))?
                .to_string(),
        ),
        ValueRef::Blob(bytes) => DbValue::Blob(bytes.to_vec()),
        ValueRef::Real(_) => {
            return Err(DbError::Sql("unexpected REAL value from sqlite".to_string()));
        }
    })
}

/// Map a SQLite declared-type string to a generic [`DbType`].
///
/// We recognize only the classes the engine actually models — INTEGER / TEXT /
/// BLOB — using SQLite's substring affinity rules. `Uuid` is indistinguishable
/// from `Blob` once stored, so it surfaces as `Blob`; callers reconcile that
/// against their own schema. Anything else (`REAL`, `NUMERIC`, a typeless or
/// unknown column) maps to [`DbType::Other`]: an honest "the engine doesn't
/// model this" rather than silently coercing it to `Blob`, so schema
/// reconciliation rejects the table instead of accepting a wrong type.
fn affinity(declared_type: &str) -> DbType {
    let t = declared_type.to_ascii_uppercase();
    if t.contains("INT") {
        DbType::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        DbType::Text
    } else if t.contains("BLOB") {
        DbType::Blob
    } else {
        DbType::Other
    }
}

/// Translate a rusqlite error into a [`DbError`], mapping UNIQUE / PRIMARY KEY
/// constraint failures to [`DbError::UniqueViolation`] so op-log ingestion can
/// detect an already-ingested entry.
fn map_err(err: rusqlite::Error) -> DbError {
    if let rusqlite::Error::SqliteFailure(e, _) = &err
        && (e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            || e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY)
    {
        return DbError::UniqueViolation;
    }
    DbError::Sql(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    // SQLite's futures resolve synchronously, so any executor drives them;
    // `pollster` is a minimal, wakeup-correct `block_on`.
    use pollster::block_on;

    fn setup() -> SqliteDb {
        let db = SqliteDb::open_in_memory().unwrap();
        block_on(db.exec(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, data BLOB) WITHOUT ROWID",
            &[],
        ))
        .unwrap();
        db
    }

    #[test]
    fn dialect_is_sqlite() {
        let db = SqliteDb::open_in_memory().unwrap();
        assert_eq!(db.dialect(), SqlDialect::Sqlite);
        assert_eq!(db.new_batch().dialect(), SqlDialect::Sqlite);
    }

    #[test]
    fn exec_and_query_roundtrip() {
        let db = setup();
        let affected = block_on(db.exec(
            "INSERT INTO t (id, name, data) VALUES (?1, ?2, ?3)",
            &[
                DbValue::Integer(1),
                DbValue::Text("alice".into()),
                DbValue::Blob(vec![1, 2, 3]),
            ],
        ))
        .unwrap();
        assert_eq!(affected, 1);

        let rows = block_on(db.query(
            "SELECT id, name, data FROM t WHERE id = ?1",
            &[DbValue::Integer(1)],
        ))
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_i64(0).unwrap(), 1);
        assert_eq!(rows[0].get_text(1).unwrap(), "alice");
        assert_eq!(rows[0].get_blob(2).unwrap(), &[1, 2, 3]);
    }

    #[test]
    fn uuid_roundtrips_as_blob() {
        let db = SqliteDb::open_in_memory().unwrap();
        block_on(db.exec("CREATE TABLE u (id BLOB PRIMARY KEY)", &[])).unwrap();
        let id = [7u8; 16];
        block_on(db.exec("INSERT INTO u (id) VALUES (?1)", &[DbValue::Uuid(id)])).unwrap();
        let rows = block_on(db.query("SELECT id FROM u", &[])).unwrap();
        assert_eq!(rows[0].get_uuid(0).unwrap(), id);
    }

    #[test]
    fn describe_table_reports_pk_and_cols() {
        let db = setup();
        let desc = block_on(db.describe_table("t")).unwrap().unwrap();
        assert_eq!(desc.name, "t");

        assert_eq!(desc.pk_cols.len(), 1);
        assert_eq!(desc.pk_cols[0].name, "id");
        assert_eq!(desc.pk_cols[0].db_type, DbType::Integer);

        let names: Vec<_> = desc.cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["name", "data"]);
        assert_eq!(desc.cols[0].db_type, DbType::Text);
        assert!(!desc.cols[0].nullable);
        assert!(desc.cols[1].nullable);
    }

    #[test]
    fn describe_missing_table_is_none() {
        let db = SqliteDb::open_in_memory().unwrap();
        assert!(block_on(db.describe_table("nope")).unwrap().is_none());
    }

    #[test]
    fn composite_pk_is_ordered() {
        let db = SqliteDb::open_in_memory().unwrap();
        block_on(db.exec(
            "CREATE TABLE c (a INTEGER, b TEXT, v BLOB, PRIMARY KEY (a, b))",
            &[],
        ))
        .unwrap();
        let desc = block_on(db.describe_table("c")).unwrap().unwrap();
        let pk: Vec<_> = desc.pk_cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(pk, vec!["a", "b"]);
    }

    #[test]
    fn update_and_delete_report_affected_rows() {
        let db = setup();
        for id in 1..=3 {
            block_on(db.exec(
                "INSERT INTO t (id, name) VALUES (?1, 'x')",
                &[DbValue::Integer(id)],
            ))
            .unwrap();
        }
        let updated =
            block_on(db.exec("UPDATE t SET name = 'y' WHERE id <= 2", &[])).unwrap();
        assert_eq!(updated, 2);
        let deleted = block_on(db.exec("DELETE FROM t", &[])).unwrap();
        assert_eq!(deleted, 3);
    }

    #[test]
    fn conditional_upsert_reports_applied() {
        // The real use case: an LWW upsert with a WHERE guard. `rows_affected`
        // must be 1 when the row is written and 0 when the WHERE holds it off —
        // even with a DDL statement run in between. Each upsert is itself DML and
        // resets `changes()`, so the answer tracks the upsert, not the preceding
        // DDL (whose own stale `rows_affected` we deliberately don't consult).
        let db = SqliteDb::open_in_memory().unwrap();
        block_on(db.exec(
            "CREATE TABLE lww (pk INTEGER PRIMARY KEY, val TEXT, ts INTEGER)",
            &[],
        ))
        .unwrap();

        let upsert = "INSERT INTO lww (pk, val, ts) VALUES (?1, ?2, ?3) \
             ON CONFLICT(pk) DO UPDATE SET val = excluded.val, ts = excluded.ts \
             WHERE excluded.ts > lww.ts";
        let up = |val: &str, ts: i64| {
            block_on(db.exec(
                upsert,
                &[DbValue::Integer(1), DbValue::Text(val.into()), DbValue::Integer(ts)],
            ))
        };

        // First write inserts.
        assert_eq!(up("a", 5).unwrap(), 1);
        // A DDL in between leaves a stale changes()==1 on the connection...
        block_on(db.exec("CREATE TABLE scratch (x INTEGER)", &[])).unwrap();
        // ...but the older-timestamp upsert is a no-op (WHERE fails) and, being
        // DML, resets the count itself → 0 applied, not the stale 1.
        assert_eq!(up("stale", 3).unwrap(), 0);
        // A newer timestamp wins → applied.
        assert_eq!(up("b", 9).unwrap(), 1);

        let rows = block_on(db.query("SELECT val FROM lww WHERE pk = 1", &[])).unwrap();
        assert_eq!(rows[0].get_text(0).unwrap(), "b");
    }

    #[test]
    fn affinity_maps_declared_types() {
        let db = SqliteDb::open_in_memory().unwrap();
        // `BLOB(16)`/`VARCHAR(8)` are substring matches per SQLite affinity; REAL
        // and NUMERIC are types the engine doesn't model → Other.
        block_on(db.exec(
            "CREATE TABLE m (a INTEGER, s VARCHAR(8), b BLOB(16), r REAL, n NUMERIC)",
            &[],
        ))
        .unwrap();
        let desc = block_on(db.describe_table("m")).unwrap().unwrap();
        let by_name: std::collections::HashMap<_, _> =
            desc.cols.iter().map(|c| (c.name.as_str(), c.db_type)).collect();
        assert_eq!(by_name["a"], DbType::Integer);
        assert_eq!(by_name["s"], DbType::Text);
        assert_eq!(by_name["b"], DbType::Blob);
        assert_eq!(by_name["r"], DbType::Other);
        assert_eq!(by_name["n"], DbType::Other);
    }

    #[test]
    fn unique_violation_is_mapped() {
        let db = setup();
        let stmt = "INSERT INTO t (id, name) VALUES (1, 'a')";
        block_on(db.exec(stmt, &[])).unwrap();
        let err = block_on(db.exec(stmt, &[])).unwrap_err();
        assert!(matches!(err, DbError::UniqueViolation), "got {err:?}");
    }

    #[test]
    fn batch_commits_atomically_with_returning() {
        let db = setup();
        let mut batch = db.new_batch();
        let s0 = batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2) RETURNING id",
            &[DbValue::Integer(10), DbValue::Text("x".into())],
        );
        let s1 = batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2)",
            &[DbValue::Integer(11), DbValue::Text("y".into())],
        );
        let results = block_on(batch.commit()).unwrap();

        assert_eq!(results[s0.0].rows_affected, 1);
        assert_eq!(results[s0.0].rows[0].get_i64(0).unwrap(), 10);
        assert_eq!(results[s1.0].rows_affected, 1);
        assert!(results[s1.0].rows.is_empty());

        let rows = block_on(db.query("SELECT COUNT(*) FROM t", &[])).unwrap();
        assert_eq!(rows[0].get_i64(0).unwrap(), 2);
    }

    #[test]
    fn batch_rolls_back_on_error() {
        let db = setup();
        let mut batch = db.new_batch();
        batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2)",
            &[DbValue::Integer(20), DbValue::Text("ok".into())],
        );
        // Duplicate id within the same batch: the second insert fails, so the
        // whole batch rolls back.
        batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2)",
            &[DbValue::Integer(20), DbValue::Text("dup".into())],
        );
        let err = block_on(batch.commit()).unwrap_err();
        assert!(matches!(err, DbError::UniqueViolation), "got {err:?}");

        let rows = block_on(db.query("SELECT COUNT(*) FROM t", &[])).unwrap();
        assert_eq!(rows[0].get_i64(0).unwrap(), 0);
    }
}
