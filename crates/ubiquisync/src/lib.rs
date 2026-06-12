//! Ubiquisync — conflict-free sync of structured data and rich text over
//! commodity cloud storage (Google Drive, iCloud Drive, Dropbox, ...) or a
//! dedicated sync server.
//!
//! This crate is the user-facing entry point. It re-exports the protocol and
//! engine from `ubiquisync-core` and exposes storage backends behind feature
//! flags (`sqlite`, enabled by default).

pub use ubiquisync_core::*;

/// SQLite storage backend (the `sqlite` feature, enabled by default).
#[cfg(feature = "sqlite")]
pub use ubiquisync_sqlite as sqlite;
