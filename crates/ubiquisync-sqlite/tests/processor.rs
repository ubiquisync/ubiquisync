//! Runs the backend-agnostic processor suite from `ubiquisync-sql` against the
//! real SQLite driver. The scenarios live in `ubiquisync_sql::test_support` so
//! every backend asserts identical behavior; this file only supplies the `Db`.

use ubiquisync_sql::test_support::run_max_register_suite;
use ubiquisync_sqlite::SqliteDb;

#[test]
fn max_register_suite() {
    run_max_register_suite(SqliteDb::open_in_memory().unwrap());
}
