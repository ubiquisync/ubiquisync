//! Runs the backend-agnostic table-reducer suite from `ubiquisync-tables`
//! against the real Postgres driver. The scenarios live in
//! `ubiquisync_tables::test_support` so every backend asserts identical
//! behavior; this file only supplies the `Db`.

mod common;

use ubiquisync_tables::test_support::run_reducer_suite;

#[tokio::test]
async fn reducer_suite() {
    let (_pg, db) = common::fresh_db().await;
    run_reducer_suite(db).await;
}
