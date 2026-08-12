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

pub trait IndexableOp: Op {
    fn to_index_entry(&self) -> Result<Vec<OpIndexEntry>, CodecError>;
    fn from_index_parts(index_entries: &[OpIndexEntry]) -> Result<Self, CodecError>;
}

pub struct OpIndexEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}
