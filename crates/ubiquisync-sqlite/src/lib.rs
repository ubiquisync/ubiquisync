//! SQLite storage backend for Ubiquisync.
//!
//! Implements the `ubiquisync-core` storage abstractions over SQLite,
//! starting with the SQL dialect ([`SqliteDialect`]). The connection layer
//! and reducer integration land as the engine port progresses.

mod dialect;

pub use dialect::SqliteDialect;
