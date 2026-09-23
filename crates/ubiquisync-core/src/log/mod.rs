mod cipher;
mod entry;
mod error;
mod hash;
mod ops;
pub mod segment;
mod transform;
mod verify;

pub use cipher::*;
pub use entry::*;
pub use error::*;
pub use hash::*;
pub use ops::*;
pub use verify::*;

use crate::{crypto::Hash256, ids::LogId};

/// Precisely defines a log entry including the hash of its parent entry
/// for precise placement into a graph of entries.
pub struct EntryOrigin {
    pub log: LogId,
    pub head: ChainHash,
    pub parent: Hash256,
}
