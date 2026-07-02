//! Synchronization traits: the read/write faces of a replica and the cursors
//! they exchange.
//!
//! - [`LogProcessor`] — write side: `apply` an entry at `(peer, index)`.
//!   Multi-writer, idempotent.
//! - [`LogSource`] — read side: `read_since` to pull, `cursors` /
//!   `watch_cursors` to publish progress.
//!
//! A store that is both is a [`Replica`] (the SQL oplog). A file log reads any
//! origin but writes only its own ([`FileLogSink`]), so it's a
//! [`FileLogReplica`] — the type-level form of "only the originating device
//! writes its own log". Both faces speak [`PeerCursors`], a per-origin version
//! vector. The convergence drivers that consume these traits come later.

mod cursors;
mod error;
mod file_log;
mod processor;
mod replica;
mod source;

pub use cursors::{CursorStream, CursorsEvent, PeerCursors};
pub use error::SyncError;
pub use file_log::{FileLogPuller, FileLogReplica, FileLogSink};
pub use processor::{Applied, LogProcessor};
pub use replica::Replica;
pub use source::LogSource;
