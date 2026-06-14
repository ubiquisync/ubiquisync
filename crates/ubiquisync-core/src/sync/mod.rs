//! Synchronization engine and its seams.
//!
//! - [`store`] — the storage seam a backend implements: read ([`LogSource`])
//!   and write ([`LogEntrySink`]) sides of a peer's log stream.
//! - [`processor`] — the apply seam ([`LogProcessor`]): what absorbs remote
//!   entries and tracks per-peer cursors.
//! - [`pull`] — the engine ([`PullSync`]) that drives a source into a processor.
//! - [`error`] — [`LogStoreError`], shared across the seams.

mod error;
mod processor;
mod pull;
mod store;

pub use error::LogStoreError;
pub use processor::LogProcessor;
pub use pull::{PullSync, SyncResult};
pub use store::{LogEntrySink, LogSource};
