//! Wire-level log entry: a single op with timestamp and optional
//! user attribution. This is the unit of encoding/decoding in a segment file.

use crate::hlc::Timestamp;
use crate::uuid::Uuid;

/// A single log entry: one operation plus metadata. This is the unit
/// written to and read from segment files — each entry has its own
/// blake3 hash and can be independently expunged.
///
/// The entry is generic over its op vocabulary: the state log carries
/// [`Op`](crate::op::Op) (system and user-defined table mutations), the
/// document log carries [`docs::op::Op`](crate::docs::op::Op). Both log
/// domains use this same envelope and share one HLC clock domain, so
/// timestamps are causally comparable across them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry<E> {
    /// User who authored this entry. `None` in device mode where
    /// attribution is implicit from the peer directory.
    pub user_id: Option<Uuid>,
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: Timestamp,
    /// The state mutation.
    pub op: E,
}
