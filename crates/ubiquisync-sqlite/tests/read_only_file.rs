//! Exercises the file-backed path (`open`): confirms WAL is actually enabled on
//! the writer, the reader sees the writer's committed rows across separate
//! connections, and the reader still rejects writes. Kept separate from the
//! in-memory suite because only a file database can enter WAL mode.

use pollster::block_on;
use ubiquisync_sql::db::{Db, DbError};
use ubiquisync_sqlite::SqliteDb;

#[test]
fn file_backed_wal_reader() {
    // A process-unique scratch path under the OS temp dir, cleaned up at the end.
    let path = std::env::temp_dir().join(format!("ubq-sqlite-wal-{}.db", std::process::id()));
    let wal = std::path::PathBuf::from(format!("{}-wal", path.display()));
    let shm = std::path::PathBuf::from(format!("{}-shm", path.display()));
    for p in [&path, &wal, &shm] {
        let _ = std::fs::remove_file(p);
    }

    let db = SqliteDb::open(&path).unwrap();
    block_on(db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", &[])).unwrap();
    block_on(db.exec("INSERT INTO t (id, v) VALUES (1, 'a')", &[])).unwrap();

    // The reader is a separate connection: seeing this row proves cross-connection
    // visibility, and the `-wal` sidecar proves the writer really entered WAL.
    let rows = block_on(db.query("SELECT v FROM t WHERE id = 1", &[])).unwrap();
    assert_eq!(rows[0].get_text(0).unwrap(), "a");
    assert!(wal.exists(), "expected a WAL file at {}", wal.display());

    // The reader still rejects writes even against a writable file DB.
    let err = block_on(db.query("INSERT INTO t (id, v) VALUES (2, 'b')", &[])).unwrap_err();
    assert!(
        matches!(err, DbError::Sql(m) if m.to_ascii_lowercase().contains("readonly")),
        "expected a read-only rejection on the reader connection"
    );

    drop(db);
    for p in [&path, &wal, &shm] {
        let _ = std::fs::remove_file(p);
    }
}
