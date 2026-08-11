use std::io::Write;

use crate::codec::error::CodecError;

pub trait Op: Sized {
    fn decode(bytes: &[u8]) -> Result<Self, CodecError>;
    /// Encode the op. The encoded op bytes must be non-empty.
    fn encode(&self, w: &mut dyn Write) -> Result<(), CodecError>;

    /// Defines what actor the op is attributed to which restricts where and how it can appear
    /// in server and device logs. Server ops can only occur in server logs and whne user
    /// ops occur in server logs they must always occur in a batch with server_user_id defined.
    /// Device and server ops should never be interpreted as emitted directly by a user.
    fn attribution(&self) -> OpAttribution {
        OpAttribution::User
    }
}

pub enum OpAttribution {
    /// Attributed to a user. In server logs, must include server_user_id.
    User,
    /// Attributed to a device only, must not appear in server logs.
    DeviceOnly,
    /// Attributed to a server, must only appear in server logs.
    ServerOnly,
    ///
    DeviceOrServer,
}

/// An [`Op`] that can also be split into an indexable `(key, value)`
/// pair for the SQL op-log. `key` is the indexable identity (such as table + primary key).
/// `value` is the op's remaining payload.
///
/// # Round-trip
///
/// [`from_index_parts`](IndexableOp::from_index_parts) must invert
/// [`to_index_entry`](IndexableOp::to_index_entry), so the full op can be
/// reconstructed from its stored parts.
pub trait IndexableOp: Op {
    /// Split `self` into its `(key, value)` pair.
    fn to_index_entry(&self) -> Result<(Vec<u8>, Vec<u8>), CodecError>;
    /// Reconstruct an op from a key, value pair. The inverse of
    /// [`to_index_entry`](IndexableOp::to_index_entry).
    fn from_index_parts(key: &[u8], value: &[u8]) -> Result<Self, CodecError>;
}
