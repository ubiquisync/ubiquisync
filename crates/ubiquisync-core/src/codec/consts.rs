//! Envelope-level wire constants, shared across all op vocabularies. The
//! op-specific tags live alongside each domain's `Op` implementation.

/// Segment mode flag: device mode — attribution is implicit from the peer
/// directory, so entries carry no user id.
pub const FLAG_DEVICE: u8 = 0;
/// Segment mode flag: server mode — every entry carries an explicit user id.
pub const FLAG_SERVER: u8 = 1;

pub const ENTRY_TYPE_OP_BATCH: u8 = 0x00;
pub const ENTRY_TYPE_USE_KEY: u8 = 0x01;
pub const ENTRY_TYPE_SIGNATURE: u8 = 0x02;
pub const ENTRY_TYPE_EXPUNGED: u8 = 0x03;

pub const SENTINEL_EXPUNGED: u8 = 0x0;

pub const SIG_ED25519: u8 = 0x0;
pub const SIG_P256: u8 = 0x1;
