//! Runs the backend-agnostic physical-schema suite from `ubiquisync-tables`
//! against the real Postgres driver. The scenarios live in
//! `ubiquisync_tables::test_support` so every backend asserts identical
//! behavior; this file only supplies the `Db`.

mod common;

use ubiquisync_tables::test_support::run_physical_schema_suite;

#[tokio::test]
async fn physical_schema_suite() {
    let (_pg, db) = common::fresh_db().await;
    run_physical_schema_suite(db).await;
}
