//! Runs the backend-agnostic physical-schema suite from `ubiquisync-tables`
//! against the real SQLite driver. The scenarios live in
//! `ubiquisync_tables::test_support` so every backend asserts identical
//! behavior; this file only supplies the `Db`.

use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::test_support::run_physical_schema_suite;

#[tokio::test]
async fn physical_schema_suite() {
    run_physical_schema_suite(SqliteDb::open_in_memory().unwrap()).await;
}
