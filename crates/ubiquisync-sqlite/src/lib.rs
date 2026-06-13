//! SQLite storage backend for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! Implements the `ubiquisync-tables` storage abstractions over SQLite,
//! starting with the SQL dialect ([`SqliteDialect`]). The connection layer
//! and reducer integration land as the engine port progresses.

mod dialect;

pub use dialect::SqliteDialect;
