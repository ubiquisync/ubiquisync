//! Runs the backend-agnostic processor suites from `ubiquisync-sql` against the
//! real Postgres driver. The scenarios live in `ubiquisync_sql::test_support` so
//! every backend asserts identical behavior; this file only supplies the `Db`.

mod common;

use ubiquisync_sql::test_support::{run_max_register_suite, run_pull_sync_suite};

#[tokio::test]
async fn max_register_suite() {
    let (_pg, db) = common::fresh_db().await;
    run_max_register_suite(db).await;
}

#[tokio::test]
async fn pull_sync_suite() {
    let (_pg, db) = common::fresh_db().await;
    run_pull_sync_suite(db).await;
}
