//! Envelope-level wire constants, shared across all op vocabularies. The
//! op-specific tags live alongside each domain's `Op` implementation.

/// Segment mode flag: device mode — attribution is implicit from the peer
/// directory, so entries carry no user id.
pub const FLAG_DEVICE: u8 = 0;
/// Segment mode flag: server mode — every entry carries an explicit user id.
pub const FLAG_SERVER: u8 = 1;

/// Reserved entry tag marking an expunged entry: the tag followed by the
/// 32-byte blake3 hash of the original entry, with no body, no timestamp
/// delta, and no integrity-check suffix. Domain op vocabularies must not reuse this tag.
pub const TAG_EXPUNGED: u8 = 0xFF;
