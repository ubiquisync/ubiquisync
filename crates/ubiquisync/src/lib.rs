//! Ubiquisync — conflict-free sync of structured data over commodity cloud
//! storage (Google Drive, iCloud Drive, Dropbox, ...) or a dedicated sync
//! server.
//!
//! > **⚠ PRE-ALPHA — WORK IN PROGRESS ⚠**
//! >
//! > This crate is in active, early development. APIs are incomplete, unproven,
//! > and **will change without notice**. Do not use it in production. Breaking
//! > changes may land on any commit.
//!
//! This crate is the user-facing entry point. It re-exports the engine from
//! `ubiquisync-core`, the table protocol from `ubiquisync-tables`, and exposes
//! storage backends behind feature flags (`sqlite`, enabled by default).

pub use ubiquisync_core::*;
// Re-exported as `table_codec` so it doesn't collide with core's `codec`.
pub use ubiquisync_tables::{codec as table_codec, dialect, id, op};

/// SQLite storage backend (the `sqlite` feature, enabled by default).
#[cfg(feature = "sqlite")]
pub use ubiquisync_sqlite as sqlite;

/// Postgres storage backend (the `postgres` feature).
#[cfg(feature = "postgres")]
pub use ubiquisync_postgres as postgres;
