//! Runs the backend-agnostic table-reducer suite from `ubiquisync-tables`
//! against the real SQLite driver. The scenarios live in
//! `ubiquisync_tables::test_support` so every backend asserts identical
//! behavior; this file only supplies the `Db`.

use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::test_support::run_reducer_suite;

#[tokio::test]
async fn reducer_suite() {
    run_reducer_suite(SqliteDb::open_in_memory().unwrap()).await;
}
