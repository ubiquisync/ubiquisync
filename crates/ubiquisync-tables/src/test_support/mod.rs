//! Backend-agnostic test suites that a driver crate runs against its real
//! [`Db`](ubiquisync_sql::db::Db).
//!
//! Each suite is an `async fn` generic over `<D: Db>`: a driver crate (e.g.
//! `ubiquisync-sqlite`) hands it a freshly opened database from a
//! `#[tokio::test]`. Keeping the
//! scenarios here — rather than duplicated in each backend's tests — is what
//! makes every backend assert identical behavior.

pub mod macros;
pub mod physical_schema;
pub mod reducer;

pub use macros::run_macros_suite;
pub use physical_schema::run_physical_schema_suite;
pub use reducer::run_reducer_suite;
