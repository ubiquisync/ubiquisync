use std::{any::Any, borrow::Borrow};

use thiserror::Error;

use crate::{
    ContainerId, PeerId,
    codec::{
        decoder::{DecodeError, decode_op_header},
        op::{EncodableOp, Op},
    },
    log_entry::{OpBatch, OpHeader, PlaintextBytes, PlaintextOpBatch},
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
        batches: &[IndexedOpBatch<Self::Op, OpHeader>],
    ) -> Result<(), DeliverError>;
}

#[derive(Error, Debug)]
#[error("deliver error")]
pub struct DeliverError;

pub struct IndexedOpBatch<O, H> {
    pub index: u64,
    pub batch: OpBatch<O, H>, // TODO decode header in advance
}

pub struct ReducerWrapper<R: Reducer> {
    reducer: R,
}

pub trait ReducerManager {
    fn deliver(
        &self,
        container_id: &ContainerId,
        peer_id: &PeerId,
        batches: &[IndexedOpBatch<PlaintextBytes, PlaintextBytes>],
    ) -> Result<Vec<OpIndexData>, ReducerError>;
}

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
        batches: &[IndexedOpBatch<PlaintextBytes, PlaintextBytes>],
    ) -> Result<Vec<OpIndexData>, ReducerError> {
        // TODO verify op attribution - can we extract server flag from the PeerId itself?
        for batch in batches {
            batch.batch.transform(
                |op_bytes, h| {
                    let op = R::Op::decode(op_bytes.borrow())?;
                    let attribution = op.attribution();
                    match attribution {
                        crate::codec::op::OpAttribution::User => todo!(),
                        crate::codec::op::OpAttribution::DeviceOnly => todo!(),
                        crate::codec::op::OpAttribution::ServerOnly => todo!(),
                        crate::codec::op::OpAttribution::DeviceOrServer => todo!(),
                    }
                    Ok(op)
                },
                |h| decode_op_header(h.borrow()),
            )?;
        }
        todo!()
    }
}
