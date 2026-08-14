use std::{any::Any, borrow::Borrow};

use thiserror::Error;

use crate::{
    codec::{
        decoder::DecodeError,
        op::{EncodableOp, Op},
    },
    ids::{ContainerId, PeerId},
    log_entry::{OpHeader, PlaintextBytes},
};

pub trait ReducerResolver {
    fn resolve_reducer(&self, container_id: &ContainerId) -> Option<&dyn ReducerManager>;
}

pub trait Reducer {
    type Op: Op + Any + 'static;

    fn deliver_ops(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[ReducerOpBatch<Self::Op>],
    ) -> Result<(), DeliverError>;
}

// NOTE: we explicitly exclude the entry idx here because we DO NOT want reducers
// relying on entry indexes for logic - because forks can exist and are tolerated
// at the reducer layer, we don't want reducers depending on indexes which may
// not be monotonic.
pub struct ReducerOpBatch<Op> {
    pub header: OpHeader,
    pub ops: Vec<Op>,
}

#[derive(Error, Debug)]
#[error("deliver error")]
pub struct DeliverError;

pub struct ReducerWrapper<R: Reducer> {
    reducer: R,
}

pub trait ReducerManager {
    fn deliver(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[ReducerOpBatch<PlaintextBytes>],
    ) -> Result<Vec<OpIndexData>, ReducerError>;
}

// TODO: we have no way of knowing entry_idx so we need some alternate way of doing this
pub struct OpIndexData {
    pub entry_idx: u64,
    pub op_idx: u64,
    pub index_key: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum ReducerError {
    #[error("decode error {0}")]
    DecodeError(#[from] DecodeError),
}

impl<R: Reducer> ReducerManager for ReducerWrapper<R> {
    fn deliver(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[ReducerOpBatch<PlaintextBytes>],
    ) -> Result<Vec<OpIndexData>, ReducerError> {
        // TODO verify op attribution - can we extract server flag from the PeerId itself?
        for batch in batches {
            for op_bytes in batch.ops.iter() {
                let op = R::Op::decode(op_bytes.borrow())?;
                let attribution = op.attribution();
                match attribution {
                    crate::codec::op::OpAttribution::User => todo!(),
                    crate::codec::op::OpAttribution::DeviceOnly => todo!(),
                    crate::codec::op::OpAttribution::ServerOnly => todo!(),
                    crate::codec::op::OpAttribution::DeviceOrServer => todo!(),
                }
            }
        }
        todo!()
    }
}
