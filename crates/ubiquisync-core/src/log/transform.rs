use crate::{
    bytes::{BytesWrapper, ToStatic},
    crypto::Hash256,
    log::{EntryBody, LogEntry, OpBatch, OpOrExpunge},
};

impl<X: BytesWrapper> OpBatch<X> {
    /// Transforms an OpBatch based on the provided transformer functions.
    /// This is useful for encoding/decoding while also performing hashing.
    /// See [LogEntry::transform] for argument descriptions.
    pub fn transform<X2: BytesWrapper, A, B, C, S, Err>(
        &self,
        entry_idx: u64,
        init_state: A,
        transform_slot: B,
        on_expunge_op: C,
    ) -> Result<(OpBatch<X2>, S), Err>
    where
        A: Fn(u64, &OpBatch<X>) -> Result<S, Err>,
        B: Fn(&X, &mut S) -> Result<X2, Err>,
        C: Fn(&Hash256, &mut S) -> Result<(), Err>,
    {
        let mut state = init_state(entry_idx, self)?;
        let timestamp = transform_slot(&self.timestamp, &mut state)?;
        let server_attested_user_id = if !self.server_attested_user_id.is_empty() {
            transform_slot(&self.server_attested_user_id, &mut state)?
        } else {
            Default::default()
        };
        let mut ops = vec![];
        for op in self.ops.iter() {
            ops.push(match op {
                OpOrExpunge::Op(op) => OpOrExpunge::Op(transform_slot(op, &mut state)?),
                OpOrExpunge::Expunge(hash) => {
                    on_expunge_op(hash, &mut state)?;
                    OpOrExpunge::Expunge(*hash)
                }
            })
        }
        Ok((
            OpBatch {
                timestamp,
                server_attested_user_id,
                ops,
            },
            state,
        ))
    }
}

impl<X: BytesWrapper> LogEntry<X> {
    /// Transforms an Entry based on the provided transformer functions.
    /// This is useful for encoding/decoding while also performing hashing.
    /// `init_state` is called at the start and its state is threaded through the remaining functions.
    /// `transform_header` is called when the header is transformed with the header and mutable state.
    /// It must return a transformed header.
    /// `transform_op` is called when an op is transformed with the op index, op, and mutable state.
    /// It must return a transformed op.
    /// `on_expunge_op` is called when an op-level expunge is encountered with the op index, hash and mutable state.
    pub fn transform<X2: BytesWrapper, A, B, C, S, Err>(
        &self,
        entry_index: u64,
        init_state: A,
        transform_slot: B,
        on_expunge_op: C,
    ) -> Result<(LogEntry<X2>, Option<S>), Err>
    where
        A: Fn(u64, &OpBatch<X>) -> Result<S, Err>,
        B: Fn(&X, &mut S) -> Result<X2, Err>,
        C: Fn(&Hash256, &mut S) -> Result<(), Err>,
    {
        Ok(match self {
            LogEntry::IndexedEntry(entry) => match entry {
                EntryBody::OpBatch(op_batch) => {
                    let (op_batch2, state) = op_batch.transform(
                        entry_index,
                        init_state,
                        transform_slot,
                        on_expunge_op,
                    )?;
                    (
                        LogEntry::IndexedEntry(EntryBody::OpBatch(op_batch2)),
                        Some(state),
                    )
                }
                EntryBody::UseKey(cipher_info) => (
                    LogEntry::IndexedEntry(EntryBody::UseKey(*cipher_info)),
                    None,
                ),
                EntryBody::Expunged(hash) => {
                    (LogEntry::IndexedEntry(EntryBody::Expunged(*hash)), None)
                }
            },
            LogEntry::Signature(signature) => (LogEntry::Signature(*signature), None),
        })
    }
}

impl<B: BytesWrapper> ToStatic for OpBatch<B>
where
    B::Static: BytesWrapper,
{
    type Static = OpBatch<B::Static>;

    fn to_static(self) -> Self::Static {
        let mut ops = vec![];
        for e in self.ops {
            ops.push(match e {
                OpOrExpunge::Op(e) => OpOrExpunge::Op(e.to_static()),
                OpOrExpunge::Expunge(e) => OpOrExpunge::Expunge(e),
            });
        }
        OpBatch {
            timestamp: self.timestamp.to_static(),
            server_attested_user_id: self.server_attested_user_id.to_static(),
            ops,
        }
    }
}

impl<B: BytesWrapper> ToStatic for LogEntry<B>
where
    B::Static: BytesWrapper,
{
    type Static = LogEntry<B::Static>;

    fn to_static(self) -> Self::Static {
        match self {
            LogEntry::IndexedEntry(entry) => LogEntry::IndexedEntry(match entry {
                EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.to_static()),
                EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info),
                EntryBody::Expunged(hash) => EntryBody::Expunged(hash),
            }),
            LogEntry::Signature(signature) => LogEntry::Signature(signature),
        }
    }
}
