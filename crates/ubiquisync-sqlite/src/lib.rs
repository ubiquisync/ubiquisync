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

use std::path::Path;
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
        // fail fast if the db is already opened by another connection
        conn.busy_timeout(Duration::ZERO).map_err(map_err)?;
        // ensure that only a single connection can write to the database!
        // this makes it possible for our internal mutex locking to enforce ordering
        conn.pragma_update(None, "locking_mode", "EXCLUSIVE")
            .map_err(map_err)?;
        // ensure we're in WAL mode
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_err)?;
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

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl Db for SqliteDb {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Sqlite
    }

    async fn describe_table(&self, name: &str) -> Result<Option<DbTableDescriptor>, DbError> {
        let conn = self.lock();

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
        let conn = self.lock();
        let result = run_statement(&conn, sql, params, false)?;
        Ok(result.rows_affected)
    }

    async fn exec_returning(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        let conn = self.lock();
        let result = run_statement(&conn, sql, params, false)?;
        Ok(result.rows)
    }

    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        let conn = self.lock();
        let result = run_statement(&conn, sql, params, true)?;
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

#[async_trait]
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
            results.push(run_statement(&txn, sql, params, false)?);
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
    readonly: bool,
) -> Result<DbStatementResult, DbError> {
    let mut stmt = conn.prepare(sql).map_err(map_err)?;
    let col_count = stmt.column_count();
    // SQLite's `readonly()` only means "does not write to the database file",
    // so it also passes transaction control (`BEGIN`, `SAVEPOINT`, ...) and
    // some setting changes, which would alter the shared connection. A read
    // always returns at least one column, so a statement with none is not one.
    //
    // Known gap: PRAGMAs that change a setting are not stopped. Those that
    // also return a row (e.g. `PRAGMA locking_mode = NORMAL`,
    // `PRAGMA max_page_count = N`, `PRAGMA busy_timeout = N`) are not
    // rejected. Those that return nothing (e.g. `PRAGMA query_only = ON`,
    // `PRAGMA foreign_keys = OFF`) are rejected here, but SQLite has already
    // applied them while preparing the statement. SQL given to `query` is
    // trusted; this is a guard against mistakes, not a security boundary.
    if readonly && (!stmt.readonly() || col_count == 0) {
        return Err(DbError::ReadonlyViolation);
    }

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
            return Err(DbError::Sql(
                "unexpected REAL value from sqlite".to_string(),
            ));
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
    use test_case::test_case;

    use super::*;

    async fn setup() -> SqliteDb {
        let db = SqliteDb::open_in_memory().unwrap();
        db.exec(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, data BLOB) WITHOUT ROWID",
            &[],
        )
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn dialect_is_sqlite() {
        let db = SqliteDb::open_in_memory().unwrap();
        assert_eq!(db.dialect(), SqlDialect::Sqlite);
        assert_eq!(db.new_batch().dialect(), SqlDialect::Sqlite);
    }

    #[tokio::test]
    async fn exec_and_query_roundtrip() {
        let db = setup().await;
        let affected = db
            .exec(
                "INSERT INTO t (id, name, data) VALUES (?1, ?2, ?3)",
                &[
                    DbValue::Integer(1),
                    DbValue::Text("alice".into()),
                    DbValue::Blob(vec![1, 2, 3]),
                ],
            )
            .await
            .unwrap();
        assert_eq!(affected, 1);

        let rows = db
            .query(
                "SELECT id, name, data FROM t WHERE id = ?1",
                &[DbValue::Integer(1)],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get_i64(0).unwrap(), 1);
        assert_eq!(rows[0].get_text(1).unwrap(), "alice");
        assert_eq!(rows[0].get_blob(2).unwrap(), &[1, 2, 3]);
    }

    #[tokio::test]
    async fn uuid_roundtrips_as_blob() {
        let db = SqliteDb::open_in_memory().unwrap();
        db.exec("CREATE TABLE u (id BLOB PRIMARY KEY)", &[])
            .await
            .unwrap();
        let id = [7u8; 16];
        db.exec("INSERT INTO u (id) VALUES (?1)", &[DbValue::Uuid(id)])
            .await
            .unwrap();
        let rows = db.query("SELECT id FROM u", &[]).await.unwrap();
        assert_eq!(rows[0].get_uuid(0).unwrap(), id);
    }

    #[tokio::test]
    async fn describe_table_reports_pk_and_cols() {
        let db = setup().await;
        let desc = db.describe_table("t").await.unwrap().unwrap();
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

    #[tokio::test]
    async fn describe_missing_table_is_none() {
        let db = SqliteDb::open_in_memory().unwrap();
        assert!(db.describe_table("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn composite_pk_is_ordered() {
        let db = SqliteDb::open_in_memory().unwrap();
        db.exec(
            "CREATE TABLE c (a INTEGER, b TEXT, v BLOB, PRIMARY KEY (a, b))",
            &[],
        )
        .await
        .unwrap();
        let desc = db.describe_table("c").await.unwrap().unwrap();
        let pk: Vec<_> = desc.pk_cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(pk, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn update_and_delete_report_affected_rows() {
        let db = setup().await;
        for id in 1..=3 {
            db.exec(
                "INSERT INTO t (id, name) VALUES (?1, 'x')",
                &[DbValue::Integer(id)],
            )
            .await
            .unwrap();
        }
        let updated = db
            .exec("UPDATE t SET name = 'y' WHERE id <= 2", &[])
            .await
            .unwrap();
        assert_eq!(updated, 2);
        let deleted = db.exec("DELETE FROM t", &[]).await.unwrap();
        assert_eq!(deleted, 3);
    }

    #[tokio::test]
    async fn conditional_upsert_reports_applied() {
        // The real use case: an LWW upsert with a WHERE guard. `rows_affected`
        // must be 1 when the row is written and 0 when the WHERE holds it off —
        // even with a DDL statement run in between. Each upsert is itself DML and
        // resets `changes()`, so the answer tracks the upsert, not the preceding
        // DDL (whose own stale `rows_affected` we deliberately don't consult).
        let db = SqliteDb::open_in_memory().unwrap();
        db.exec(
            "CREATE TABLE lww (pk INTEGER PRIMARY KEY, val TEXT, ts INTEGER)",
            &[],
        )
        .await
        .unwrap();

        let upsert = "INSERT INTO lww (pk, val, ts) VALUES (?1, ?2, ?3) \
             ON CONFLICT(pk) DO UPDATE SET val = excluded.val, ts = excluded.ts \
             WHERE excluded.ts > lww.ts";
        let up = async |val: &str, ts: i64| {
            db.exec(
                upsert,
                &[
                    DbValue::Integer(1),
                    DbValue::Text(val.into()),
                    DbValue::Integer(ts),
                ],
            )
            .await
        };

        // First write inserts.
        assert_eq!(up("a", 5).await.unwrap(), 1);
        // A DDL in between leaves a stale changes()==1 on the connection...
        db.exec("CREATE TABLE scratch (x INTEGER)", &[])
            .await
            .unwrap();
        // ...but the older-timestamp upsert is a no-op (WHERE fails) and, being
        // DML, resets the count itself → 0 applied, not the stale 1.
        assert_eq!(up("stale", 3).await.unwrap(), 0);
        // A newer timestamp wins → applied.
        assert_eq!(up("b", 9).await.unwrap(), 1);

        let rows = db
            .query("SELECT val FROM lww WHERE pk = 1", &[])
            .await
            .unwrap();
        assert_eq!(rows[0].get_text(0).unwrap(), "b");
    }

    #[tokio::test]
    async fn affinity_maps_declared_types() {
        let db = SqliteDb::open_in_memory().unwrap();
        // `BLOB(16)`/`VARCHAR(8)` are substring matches per SQLite affinity; REAL
        // and NUMERIC are types the engine doesn't model → Other.
        db.exec(
            "CREATE TABLE m (a INTEGER, s VARCHAR(8), b BLOB(16), r REAL, n NUMERIC)",
            &[],
        )
        .await
        .unwrap();
        let desc = db.describe_table("m").await.unwrap().unwrap();
        let by_name: std::collections::HashMap<_, _> = desc
            .cols
            .iter()
            .map(|c| (c.name.as_str(), c.db_type))
            .collect();
        assert_eq!(by_name["a"], DbType::Integer);
        assert_eq!(by_name["s"], DbType::Text);
        assert_eq!(by_name["b"], DbType::Blob);
        assert_eq!(by_name["r"], DbType::Other);
        assert_eq!(by_name["n"], DbType::Other);
    }

    #[tokio::test]
    async fn unique_violation_is_mapped() {
        let db = setup().await;
        let stmt = "INSERT INTO t (id, name) VALUES (1, 'a')";
        db.exec(stmt, &[]).await.unwrap();
        let err = db.exec(stmt, &[]).await.unwrap_err();
        assert!(matches!(err, DbError::UniqueViolation), "got {err:?}");
    }

    #[tokio::test]
    async fn batch_commits_atomically_with_returning() {
        let db = setup().await;
        let mut batch = db.new_batch();
        let s0 = batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2) RETURNING id",
            &[DbValue::Integer(10), DbValue::Text("x".into())],
        );
        let s1 = batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, ?2)",
            &[DbValue::Integer(11), DbValue::Text("y".into())],
        );
        let results = batch.commit().await.unwrap();

        assert_eq!(results[s0.0].rows_affected, 1);
        assert_eq!(results[s0.0].rows[0].get_i64(0).unwrap(), 10);
        assert_eq!(results[s1.0].rows_affected, 1);
        assert!(results[s1.0].rows.is_empty());

        let rows = db.query("SELECT COUNT(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows[0].get_i64(0).unwrap(), 2);
    }

    #[tokio::test]
    async fn batch_rolls_back_on_error() {
        let db = setup().await;
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
        let err = batch.commit().await.unwrap_err();
        assert!(matches!(err, DbError::UniqueViolation), "got {err:?}");

        let rows = db.query("SELECT COUNT(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows[0].get_i64(0).unwrap(), 0);
    }

    /// Insert row `id` through a batch, i.e. inside its own transaction.
    async fn batch_insert(db: &SqliteDb, id: i64) -> Result<(), DbError> {
        let mut batch = db.new_batch();
        batch.add_statement(
            "INSERT INTO t (id, name) VALUES (?1, 'x')",
            &[DbValue::Integer(id)],
        );
        batch.commit().await.map(|_| ())
    }

    // Most of these pass `Statement::readonly()` (they do not write to the
    // database file) and are caught because they return no columns. Not
    // covered, see `run_statement`: PRAGMAs that change a setting.
    #[test_case("BEGIN" ; "begin")]
    #[test_case("/* c */ BEGIN DEFERRED" ; "begin behind a comment")]
    #[test_case("BEGIN IMMEDIATE" ; "begin immediate")]
    #[test_case("COMMIT" ; "commit")]
    #[test_case("END" ; "end")]
    #[test_case("ROLLBACK" ; "rollback")]
    #[test_case("SAVEPOINT s" ; "savepoint")]
    #[test_case("RELEASE s" ; "release")]
    #[test_case("ATTACH ':memory:' AS a" ; "attach")]
    #[test_case("DETACH a" ; "detach")]
    #[test_case("REINDEX" ; "reindex")]
    #[test_case("VACUUM" ; "vacuum")]
    #[test_case("INSERT INTO t (id, name) VALUES (1, 'x') RETURNING id" ; "insert returning")]
    #[test_case("CREATE TEMP TABLE tt (x INTEGER)" ; "temp table")]
    #[tokio::test]
    async fn query_rejects_everything_but_reads(sql: &str) {
        let db = setup().await;
        let err = db.query(sql, &[]).await.unwrap_err();
        assert!(matches!(err, DbError::ReadonlyViolation), "got {err:?}");

        // The rejected statement left the connection usable: both write paths
        // still work, and reads see what they wrote.
        batch_insert(&db, 2).await.unwrap();
        db.exec("INSERT INTO t (id, name) VALUES (3, 'x')", &[])
            .await
            .unwrap();
        let rows = db.query("SELECT COUNT(*) FROM t", &[]).await.unwrap();
        assert_eq!(rows[0].get_i64(0).unwrap(), 2);
    }

    #[test_case("SELECT id FROM t WHERE id = 1" => 1 ; "select")]
    #[test_case("SELECT COUNT(*), MAX(length(name)) FROM t" => 1 ; "functions")]
    #[test_case("SELECT name FROM t UNION ALL VALUES ('y')" => 2 ; "compound select")]
    #[test_case("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3) \
                 SELECT i FROM n" => 3 ; "recursive cte")]
    #[test_case("SELECT name FROM sqlite_schema" => 1 ; "schema table")]
    #[test_case("PRAGMA table_info(t)" => 3 ; "read-only pragma")]
    #[test_case("SELECT name FROM pragma_table_info('t')" => 3 ; "pragma table function")]
    #[tokio::test]
    async fn query_allows_reads(sql: &str) -> usize {
        let db = setup().await;
        batch_insert(&db, 1).await.unwrap();
        db.query(sql, &[]).await.unwrap().len()
    }
}
