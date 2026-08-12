use std::any::Any;

use thiserror::Error;

use crate::{
    codec::op::{DynOp, DynOpParser, IndexableOp, OpParser},
    log_entry::{OpBatch, OpOrExpunge},
    uuid::Uuid,
};

pub trait ReducerResolver {
    fn resolve_reducer(&self, container_id: &Uuid) -> Option<&dyn DynReducer>;
}

pub trait Reducer {
    type Op: IndexableOp + Any + 'static;

    fn deliver_ops(
        &self,
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<Self::Op>],
    ) -> Result<(), DeliverError>;
}

#[derive(Error, Debug)]
#[error("deliver error")]
pub struct DeliverError;

pub struct IndexedOpBatch<O> {
    pub index: u64,
    pub batch: OpBatch<O>,
}

pub struct ReducerWrapper<R: Reducer> {
    parser: OpParser<R::Op>,
    reducer: R,
}

pub trait DynReducer {
    fn op_parser(&self) -> &dyn DynOpParser;
    fn deliver(
        &self,
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<Box<dyn DynOp>>],
    ) -> Result<(), DynReducerError>;
}

#[derive(Error, Debug)]
pub enum DynReducerError {
    #[error("downcast error")]
    DowncastError,
    #[error("deliver error")]
    DeliverError(#[from] DeliverError),
}

impl<R: Reducer> DynReducer for ReducerWrapper<R> {
    fn op_parser(&self) -> &dyn DynOpParser {
        &self.parser
    }

    fn deliver(
        &self,
        container_id: &Uuid,
        peer_id: &Uuid,
        batches: &[IndexedOpBatch<Box<dyn DynOp>>],
    ) -> Result<(), DynReducerError> {
        let downcasted = vec![];
        for ib in batches.iter() {
            let mut downcasted_ops = vec![];
            for op in ib.batch.ops.iter() {
                match op {
                    OpOrExpunge::Op(op) => {
                        if let Some(op) = op.as_any().downcast_ref::<R::Op>() {
                            downcasted_ops.push(OpOrExpunge::Op(op))
                        } else {
                            return Err(DynReducerError::DowncastError); // TODO better error
                        }
                    }
                    OpOrExpunge::Expunge(hash) => downcasted_ops.push(OpOrExpunge::Expunge(*hash)),
                }
            }
        }
        self.reducer
            .deliver_ops(container_id, peer_id, &downcasted)?;
        Ok(())
    }
}
