//! Verifies the reader connection is genuinely read-only: a write routed through
//! the read path is rejected by the engine, not silently applied.

use pollster::block_on;
use ubiquisync_sql::db::{Db, DbError};
use ubiquisync_sqlite::SqliteDb;

#[test]
fn reader_connection_rejects_writes() {
    let db = SqliteDb::open_in_memory().unwrap();
    block_on(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)", &[])).unwrap();

    // A write issued on the read path (`query` → reader connection) must fail the
    // session-level `query_only` guard with `SQLITE_READONLY`, even though the
    // database is writable and this process opened it read-write.
    let err = block_on(db.query("INSERT INTO t (id) VALUES (1)", &[])).unwrap_err();
    match err {
        DbError::Sql(msg) => assert!(
            msg.to_ascii_lowercase().contains("readonly"),
            "expected a read-only (SQLITE_READONLY) violation, got: {msg}"
        ),
        other => panic!("expected a Sql read-only error, got {other:?}"),
    }

    // Nothing was written, and the writer path still works.
    block_on(db.exec("INSERT INTO t (id) VALUES (1)", &[])).unwrap();
    let rows = block_on(db.query("SELECT COUNT(*) FROM t", &[])).unwrap();
    assert_eq!(rows[0].get_i64(0).unwrap(), 1);
}
