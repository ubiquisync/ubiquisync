//! Protocol-agnostic SQL primitives for Ubiquisync.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! The sync engine is storage-agnostic: it builds SQL as strings and runs them
//! through a backend connection. This crate holds the pieces of that story that
//! know nothing about any particular data domain:
//!
//! - [`db`] — the backend abstraction ([`Db`](db::Db)/[`DbBatch`](db::DbBatch)),
//!   the value/row types, and the generic [`DbType`](db::DbType) storage class.
//! - [`dialect`] — the closed [`SqlDialect`](dialect::SqlDialect) enum capturing
//!   the points where SQL flavors diverge.
//! - [`hlc_storage`] — a SQL-backed implementation of the core HLC storage.
//!
//! Data domains such as the table protocol (`ubiquisync-tables`) build on top
//! of these; concrete drivers (`ubiquisync-sqlite`, `ubiquisync-postgres`)
//! implement [`Db`](db::Db) and report which [`SqlDialect`](dialect::SqlDialect)
//! they speak.

pub mod db;
pub mod dialect;
pub mod hlc_storage;
pub mod processor;
pub mod reducer;
pub mod tracker;
pub mod util;

/// Backend-agnostic test suites the driver crates run against their real `Db`.
/// Compiled for this crate's own tests, and for any crate that enables the
/// `test-support` feature (a SQL driver, in its dev-dependencies).
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
