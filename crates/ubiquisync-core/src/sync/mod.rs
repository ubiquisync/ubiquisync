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
//! vector.
//!
//! [`FileLogPublisher`] is the first driver that consumes these traits: it
//! projects an oplog's own origin into its file log, so a local write reaches
//! shared storage. The inbound drivers come later.

mod cursors;
mod error;
mod file_log;
mod processor;
mod publisher;
mod replica;
mod source;

pub use cursors::{CursorStream, CursorsEvent, HasCursors, PeerCursors};
pub use error::SyncError;
pub use file_log::{FileLogPuller, FileLogReplica, FileLogSink};
pub use processor::{Applied, LogProcessor};
pub use publisher::FileLogPublisher;
pub use replica::Replica;
pub use source::LogSource;
