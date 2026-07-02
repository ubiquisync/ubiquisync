//! Runs the backend-agnostic processor suite from `ubiquisync-sql` against the
//! real SQLite driver. The scenarios live in `ubiquisync_sql::test_support` so
//! every backend asserts identical behavior; this file only supplies the `Db`.

use ubiquisync_sql::test_support::{run_max_register_suite, run_replica_suite};
use ubiquisync_sqlite::SqliteDb;

#[test]
fn max_register_suite() {
    // SQLite's futures are synchronous, so any executor drives the suite to
    // completion; `pollster` is a minimal, wakeup-correct `block_on`.
    pollster::block_on(run_max_register_suite(SqliteDb::open_in_memory().unwrap()));
}

#[test]
fn replica_suite() {
    pollster::block_on(run_replica_suite(SqliteDb::open_in_memory().unwrap()));
}
