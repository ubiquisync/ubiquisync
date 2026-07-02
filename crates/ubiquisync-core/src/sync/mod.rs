//! Synchronization engine and the traits it builds on.
//!
//! - [`LogSource`] / [`LogEntrySink`] — the storage traits a backend implements:
//!   the read and write sides of a peer's log stream.
//! - [`LogProcessor`] — the apply trait: what absorbs a peer's entries and
//!   tracks per-peer cursors.
//! - [`PullSynchronizer`] — the engine that drives a source into a processor.
//! - [`SyncError`] — the umbrella error shared across these traits.

mod error;
mod processor;
mod pull;
mod store;

pub use error::SyncError;
pub use processor::LogProcessor;
pub use pull::{PullSynchronizer, SyncResult};
pub use store::{LogEntrySink, LogSource};
