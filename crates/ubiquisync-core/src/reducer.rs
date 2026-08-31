use std::{any::Any, borrow::Borrow};

use thiserror::Error;

use crate::{
    BoxedError,
    ids::{ContainerId, PeerId},
    log_entry::{EncodableOp, Op, OpAttribution, OpHeader, PlaintextBytes},
};

pub trait ReducerResolver {
    fn resolve_reducer(&self, container_id: &ContainerId) -> Option<&dyn ReducerManager>;
}

pub trait Reducer {
    type Op: Op + Any + 'static;

    // TODO we should pass the async commiter directly to deliver and it must take a status
    // we should not expose actual indexes to the reducer since there could be many streams per peer
    // also we should _never_ index and batch with a failed status
    // batches should succeed/fail atomically (if ops are batchable at all)
    // so we either index the whole batch or none of it
    // note that one exception is for ObservePeers/CommitContainers in ctl -
    // those sorts of observations should never revert so one edge case to think about...
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
    DecodeError(BoxedError),
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
                let op = R::Op::decode(op_bytes.borrow()).map_err(ReducerError::DecodeError)?;
                let attribution = op.attribution();
                match attribution {
                    OpAttribution::User => todo!(),
                    OpAttribution::DeviceOnly => todo!(),
                    OpAttribution::ServerOnly => todo!(),
                    OpAttribution::DeviceOrServer => todo!(),
                }
            }
        }
        todo!()
    }
}
