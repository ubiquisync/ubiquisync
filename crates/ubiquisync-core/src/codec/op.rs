use std::{any::Any, io::Write, marker::PhantomData};

use crate::{
    codec::{PlaintextOpBatchHasher, error::CodecError},
    log_entry::{OpBatch, PlaintextOpBatch},
};

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
    fn to_index_parts(&self) -> Result<Vec<OpIndexEntry>, CodecError>;
    fn from_index_parts(index_entries: &[OpIndexEntry]) -> Result<Self, CodecError>;
}

pub struct OpIndexEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

pub trait DynOp: Any {
    fn encode(&self, w: &mut dyn Write) -> Result<(), CodecError>;
    fn attribution(&self) -> OpAttribution;
    fn as_any(&self) -> &dyn Any;
    fn to_index_parts(&self) -> Result<Vec<OpIndexEntry>, CodecError>;
}

impl<T: IndexableOp + Any> DynOp for T {
    fn encode(&self, w: &mut dyn Write) -> Result<(), CodecError> {
        Op::encode(self, w)
    }

    fn attribution(&self) -> OpAttribution {
        Op::attribution(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_index_parts(&self) -> Result<Vec<OpIndexEntry>, CodecError> {
        IndexableOp::to_index_parts(self)
    }
}

pub struct OpParser<O> {
    _phantom: PhantomData<O>,
}

pub trait DynOpParser {
    fn decode(&self, bytes: &[u8]) -> Result<Box<dyn DynOp>, CodecError>;
    fn from_index_parts(&self, index_parts: &[OpIndexEntry]) -> Result<Box<dyn DynOp>, CodecError>;
}

impl<O: IndexableOp + Any> DynOpParser for OpParser<O> {
    fn decode(&self, bytes: &[u8]) -> Result<Box<dyn DynOp>, CodecError> {
        Ok(Box::new(<O as Op>::decode(bytes)?))
    }

    fn from_index_parts(&self, index_parts: &[OpIndexEntry]) -> Result<Box<dyn DynOp>, CodecError> {
        Ok(Box::new(<O as IndexableOp>::from_index_parts(index_parts)?))
    }
}
