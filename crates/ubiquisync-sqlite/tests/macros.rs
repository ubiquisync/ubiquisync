//! Runs the backend-agnostic table-macro suite from `ubiquisync-tables` against
//! the real SQLite driver. The scenarios live in `ubiquisync_tables::test_support`
//! so every backend asserts identical behavior; this file only supplies the `Db`.

use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::test_support::run_macros_suite;

#[test]
fn macros_suite() {
    // SQLite's futures are synchronous, so any executor drives the suite to
    // completion; `pollster` is a minimal, wakeup-correct `block_on`.
    pollster::block_on(run_macros_suite(SqliteDb::open_in_memory().unwrap()));
}
