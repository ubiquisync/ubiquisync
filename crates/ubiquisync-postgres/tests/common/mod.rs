//! Shared test harness: boots a throwaway embedded PostgreSQL and hands the
//! backend-agnostic suites a freshly connected [`PostgresDb`].
//!
//! Each call to [`fresh_db`] starts its own server on an ephemeral port with a
//! temporary data directory, so the suites — which use fixed table names — never
//! collide, and teardown is automatic when the returned [`PostgreSQL`] handle is
//! dropped at the end of the test.

use postgresql_embedded::PostgreSQL;
use ubiquisync_postgres::PostgresDb;

/// The database created inside the fresh server for the suite to use.
const TEST_DB: &str = "ubiquisync_test";

/// Boot an embedded PostgreSQL, create an empty database, and connect to it.
///
/// Returns the running server alongside the connected [`PostgresDb`]. **Keep the
/// [`PostgreSQL`] handle bound for the whole test** (`let (_pg, db) = …`):
/// dropping it stops the server, which would kill `db`'s connection mid-test.
pub async fn fresh_db() -> (PostgreSQL, PostgresDb) {
    let mut postgres = PostgreSQL::default();
    postgres
        .setup()
        .await
        .expect("embedded postgres: setup (first run downloads the binary)");
    postgres.start().await.expect("embedded postgres: start");
    postgres
        .create_database(TEST_DB)
        .await
        .expect("embedded postgres: create database");

    let url = postgres.settings().url(TEST_DB);
    let db = PostgresDb::connect(&url)
        .await
        .expect("connect to embedded postgres");
    (postgres, db)
}
