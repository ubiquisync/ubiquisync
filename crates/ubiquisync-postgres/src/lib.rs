//! Postgres storage backend for Ubiquisync.
//!
//! Implements the `ubiquisync-core` storage abstractions over Postgres,
//! starting with the SQL dialect ([`PostgresDialect`]). The connection layer
//! and reducer integration land as the engine port progresses.

mod dialect;

pub use dialect::PostgresDialect;
