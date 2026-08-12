use std::any::Any;

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
    ) -> Result<(), Error>;
}

pub struct IndexedOpBatch<O> {
    pub index: u64,
    pub batch: OpBatch<O>,
}

// TODO better error type
pub struct Error;

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
    ) -> Result<(), Error>;
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
    ) -> Result<(), Error> {
        let downcasted = vec![];
        for ib in batches.iter() {
            let mut downcasted_ops = vec![];
            for op in ib.batch.ops.iter() {
                match op {
                    OpOrExpunge::Op(op) => {
                        if let Some(op) = op.as_any().downcast_ref::<R::Op>() {
                            downcasted_ops.push(OpOrExpunge::Op(op))
                        } else {
                            return Err(Error); // TODO better error
                        }
                    }
                    OpOrExpunge::Expunge(hash) => downcasted_ops.push(OpOrExpunge::Expunge(*hash)),
                }
            }
        }
        self.reducer.deliver_ops(container_id, peer_id, &downcasted)
    }
}
