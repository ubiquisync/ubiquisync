//! Runs the backend-agnostic table-macro suite from `ubiquisync-tables` against
//! the real SQLite driver. The scenarios live in `ubiquisync_tables::test_support`
//! so every backend asserts identical behavior; this file only supplies the `Db`.

use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::test_support::run_macros_suite;

#[tokio::test]
async fn macros_suite() {
    run_macros_suite(SqliteDb::open_in_memory().unwrap()).await;
}
