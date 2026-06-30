//! SQLite storage backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate implements the [`Db`](ubiquisync_sql::db::Db) backend abstraction
//! over [`rusqlite`], reporting
//! [`SqlDialect::Sqlite`](ubiquisync_sql::dialect::SqlDialect::Sqlite). The SQL
//! flavor itself is not implemented here — it lives in `ubiquisync-sql`; this
//! crate only drives the connection.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, params_from_iter};

use ubiquisync_sql::db::{
    Db, DbBatch, DbColumnDescription, DbError, DbRow, DbStatementResult, DbTableDescriptor, DbType,
    DbValue, StmtId,
};
use ubiquisync_sql::dialect::SqlDialect;

/// A [`Db`] backed by a rusqlite [`Connection`].
///
/// The connection is wrapped in an [`Arc<Mutex<_>>`] so that batches handed out
/// by [`new_batch`](Db::new_batch) can run against the same database. SQLite has
/// a single write slot anyway, so serializing access behind one mutex matches
/// the engine and costs nothing in contention we wouldn't already pay.
#[derive(Clone)]
pub struct SqliteDb {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteDb {
    /// Open (creating if needed) a database file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let conn = Connection::open(path).map_err(map_err)?;
        Ok(Self::from_connection(conn))
    }

    /// Open a fresh, private in-memory database. Useful for tests.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory().map_err(map_err)?;
        Ok(Self::from_connection(conn))
    }

    /// Wrap an already-open [`Connection`].
    pub fn from_connection(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Acquire the connection, mapping a poisoned mutex to a [`DbError`] rather
    /// than panicking. A poison means a prior caller panicked mid-statement, so
    /// the connection may be mid-transaction; we surface that as an error and
    /// let the caller decide, instead of crashing the whole process.
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, DbError> {
        self.conn
            .lock()
            .map_err(|_| DbError::Sql("sqlite connection mutex poisoned".to_string()))
    }
}

#[async_trait(?Send)]
impl Db for SqliteDb {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    async fn describe_table(&self, name: &str) -> Result<Option<DbTableDescriptor>, DbError> {
        let conn = self.lock()?;

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
        let conn = self.lock()?;
        let result = run_statement(&conn, sql, params)?;
        Ok(result.rows_affected)
    }

    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        let conn = self.lock()?;
        let result = run_statement(&conn, sql, params)?;
        Ok(result.rows)
    }

    fn new_batch(&self) -> Box<dyn DbBatch> {
        Box::new(SqliteBatch {
            conn: Arc::clone(&self.conn),
            statements: Vec::new(),
        })
    }
}

/// An atomic batch of writes against a [`SqliteDb`].
struct SqliteBatch {
    conn: Arc<Mutex<Connection>>,
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
        let conn = self
            .conn
            .lock()
            .map_err(|_| DbError::Sql("sqlite connection mutex poisoned".to_string()))?;

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
/// `rows_affected` is `Connection::changes()` — the direct row count of the most
/// recent INSERT/UPDATE/DELETE, excluding trigger/cascade rows — gated so that a
/// statement which modified no rows reports 0.
///
/// `changes()` is the right primitive for the caller's question "did this
/// conditional upsert apply?": it is 1 when the target row was written (inserted,
/// or conflict-updated with its `WHERE` passing) and 0 when the `WHERE` held the
/// write off. But `changes()` is *not reset* by DDL or SELECT, so a `CREATE
/// TABLE`/`SELECT` run after a prior write would echo that write's count. An
/// upsert is itself DML and so resets `changes()` correctly even when preceded by
/// DDL; the stale read only affects non-DML statements, whose true count is 0. We
/// detect "did this statement modify any rows" via the monotonic cumulative
/// counter and report 0 when it did not — fixing DDL/SELECT without inflating the
/// write count the way a raw cumulative delta would when triggers fire.
fn run_statement(
    conn: &Connection,
    sql: &str,
    params: &[DbValue],
) -> Result<DbStatementResult, DbError> {
    let mut stmt = conn.prepare(sql).map_err(map_err)?;
    let col_count = stmt.column_count();

    let total_before = total_changes(conn);
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

    // Gate `changes()` on whether this statement modified any rows at all, so a
    // non-DML statement reports 0 rather than a previous write's stale count.
    let rows_affected = if total_changes(conn) != total_before {
        conn.changes() as usize
    } else {
        0
    };
    Ok(DbStatementResult {
        rows_affected,
        rows: rows_out,
    })
}

/// The cumulative number of rows inserted/updated/deleted on this connection
/// since it was opened (`sqlite3_total_changes`). Unlike `Connection::changes()`
/// it is monotonic and not reset by DDL/SELECT, so a change across a statement
/// tells us whether that statement modified any rows. rusqlite 0.31 has no safe
/// wrapper, so we call the ffi binding directly.
fn total_changes(conn: &Connection) -> i64 {
    // SAFETY: `handle()` returns this connection's live `sqlite3*`, valid for the
    // duration of the borrow; `sqlite3_total_changes` only reads a counter off it
    // and never mutates or stores the pointer.
    unsafe { rusqlite::ffi::sqlite3_total_changes(conn.handle()) as i64 }
}

/// Map a [`DbValue`] parameter to a rusqlite value. UUIDs are stored as their 16
/// raw bytes, matching [`DbType::Uuid`]'s `BLOB` storage class.
fn to_sql_value(value: &DbValue) -> SqlValue {
    match value {
        DbValue::Null => SqlValue::Null,
        DbValue::Integer(i) => SqlValue::Integer(*i),
        DbValue::Text(s) => SqlValue::Text(s.clone()),
        DbValue::Blob(b) => SqlValue::Blob(b.clone()),
        DbValue::Uuid(u) => SqlValue::Blob(u.to_vec()),
    }
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

/// Map the SQLite declared-type string to a generic [`DbType`] via SQLite's
/// column-affinity rules (the subset the engine actually emits: INTEGER / TEXT /
/// BLOB). `Uuid` is indistinguishable from `Blob` once stored, so it surfaces as
/// `Blob`; callers reconcile that against their own schema.
fn affinity(declared_type: &str) -> DbType {
    let t = declared_type.to_ascii_uppercase();
    if t.contains("INT") {
        DbType::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        DbType::Text
    } else {
        DbType::Blob
    }
}

/// Translate a rusqlite error into a [`DbError`], mapping UNIQUE / PRIMARY KEY
/// constraint failures to [`DbError::UniqueViolation`] so op-log ingestion can
/// detect an already-ingested entry.
fn map_err(err: rusqlite::Error) -> DbError {
    if let rusqlite::Error::SqliteFailure(e, _) = &err {
        if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            || e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        {
            return DbError::UniqueViolation;
        }
    }
    DbError::Sql(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    /// Minimal executor for the tests: every future in this crate resolves
    /// without ever yielding (the bodies do no real `.await`), so polling once
    /// in a loop with a no-op waker is sufficient — no async runtime needed.
    fn block_on<F: Future>(fut: F) -> F::Output {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = pin!(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

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
    fn rows_affected_not_stale_after_ddl() {
        // After a write that changed a row, a subsequent non-DML statement must
        // report 0 affected — not the prior write's count (the `changes()` trap).
        let db = setup();
        let n = block_on(db.exec("INSERT INTO t (id, name) VALUES (1, 'a')", &[])).unwrap();
        assert_eq!(n, 1);
        let n = block_on(db.exec("CREATE TABLE t2 (x INTEGER)", &[])).unwrap();
        assert_eq!(n, 0, "DDL after a write should report 0 rows affected");
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
        // even with a DDL statement run in between (the `changes()` staleness
        // trap). Each upsert resets `changes()` itself, so the answer tracks the
        // upsert, not the preceding DDL.
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
        // A DDL in between would leave a stale changes()==1 if read naively...
        assert_eq!(
            block_on(db.exec("CREATE TABLE scratch (x INTEGER)", &[])).unwrap(),
            0
        );
        // ...but the older-timestamp upsert is a no-op: WHERE fails → 0 applied.
        assert_eq!(up("stale", 3).unwrap(), 0);
        // A newer timestamp wins → applied.
        assert_eq!(up("b", 9).unwrap(), 1);

        let rows = block_on(db.query("SELECT val FROM lww WHERE pk = 1", &[])).unwrap();
        assert_eq!(rows[0].get_text(0).unwrap(), "b");
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
