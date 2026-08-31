use crate::{
    bytes::ToStatic,
    crypto::Hash256,
    log::{EntryBody, LogEntry, OpBatch, OpOrExpunge},
};

impl<O: std::fmt::Debug, H: std::fmt::Debug> OpBatch<O, H> {
    /// Transforms an OpBatch based on the provided transformer functions.
    /// This is useful for encoding/decoding while also performing hashing.
    /// See [LogEntry::transform] for argument descriptions.
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, A, B, C, D, S, E>(
        &self,
        entry_idx: u64,
        init_state: A,
        transform_header: B,
        transform_op: C,
        on_expunge_op: D,
    ) -> Result<(OpBatch<O2, H2>, S), E>
    where
        A: Fn(u64, &OpBatch<O, H>) -> Result<S, E>,
        B: Fn(&H, &mut S) -> Result<H2, E>,
        C: Fn(u64, &O, &mut S) -> Result<O2, E>,
        D: Fn(u64, &Hash256, &mut S) -> Result<(), E>,
    {
        let mut state = init_state(entry_idx, self)?;
        let header = transform_header(&self.header, &mut state)?;
        let mut ops = vec![];
        for (idx, op) in self.ops.iter().enumerate() {
            ops.push(match op {
                OpOrExpunge::Op(op) => OpOrExpunge::Op(transform_op(idx as u64, op, &mut state)?),
                OpOrExpunge::Expunge(hash) => {
                    on_expunge_op(idx as u64, hash, &mut state)?;
                    OpOrExpunge::Expunge(*hash)
                }
            })
        }
        Ok((OpBatch { header, ops }, state))
    }
}

impl<O: std::fmt::Debug, H: std::fmt::Debug> LogEntry<O, H> {
    /// Transforms an Entry based on the provided transformer functions.
    /// This is useful for encoding/decoding while also performing hashing.
    /// `init_state` is called at the start and its state is threaded through the remaining functions.
    /// `transform_header` is called when the header is transformed with the header and mutable state.
    /// It must return a transformed header.
    /// `transform_op` is called when an op is transformed with the op index, op, and mutable state.
    /// It must return a transformed op.
    /// `on_expunge_op` is called when an op-level expunge is encountered with the op index, hash and mutable state.
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, A, B, C, D, S, E>(
        &self,
        init_state: A,
        transform_header: B,
        transform_op: C,
        on_expunge_op: D,
    ) -> Result<(LogEntry<O2, H2>, Option<S>), E>
    where
        A: Fn(u64, &OpBatch<O, H>) -> Result<S, E>,
        B: Fn(&H, &mut S) -> Result<H2, E>,
        C: Fn(u64, &O, &mut S) -> Result<O2, E>,
        D: Fn(u64, &Hash256, &mut S) -> Result<(), E>,
    {
        Ok(match self {
            LogEntry::IndexedEntry { idx, entry } => match entry {
                EntryBody::OpBatch(op_batch) => {
                    let (op_batch2, state) = op_batch.transform(
                        *idx,
                        init_state,
                        transform_header,
                        transform_op,
                        on_expunge_op,
                    )?;
                    (
                        LogEntry::IndexedEntry {
                            entry: EntryBody::OpBatch(op_batch2),
                            idx: *idx,
                        },
                        Some(state),
                    )
                }
                EntryBody::UseKey(cipher_info) => (
                    LogEntry::IndexedEntry {
                        entry: EntryBody::UseKey(*cipher_info),
                        idx: *idx,
                    },
                    None,
                ),
            },
            LogEntry::Expunged { end_size, end_hash } => (
                LogEntry::Expunged {
                    end_size: *end_size,
                    end_hash: *end_hash,
                },
                None,
            ),
            LogEntry::Signature { size, signature } => (
                LogEntry::Signature {
                    size: *size,
                    signature: *signature,
                },
                None,
            ),
        })
    }
}

impl<O: ToStatic + std::fmt::Debug, H: ToStatic + std::fmt::Debug> ToStatic for OpBatch<O, H>
where
    O::Static: std::fmt::Debug,
    H::Static: std::fmt::Debug,
{
    type Static = OpBatch<O::Static, H::Static>;

    fn to_static(self) -> Self::Static {
        let mut ops = vec![];
        for e in self.ops {
            ops.push(match e {
                OpOrExpunge::Op(e) => OpOrExpunge::Op(e.to_static()),
                OpOrExpunge::Expunge(e) => OpOrExpunge::Expunge(e),
            });
        }
        OpBatch {
            header: self.header.to_static(),
            ops,
        }
    }
}

impl<O: ToStatic + std::fmt::Debug, H: ToStatic + std::fmt::Debug> ToStatic for LogEntry<O, H>
where
    O::Static: std::fmt::Debug,
    H::Static: std::fmt::Debug,
{
    type Static = LogEntry<O::Static, H::Static>;

    fn to_static(self) -> Self::Static {
        match self {
            LogEntry::IndexedEntry { idx, entry } => LogEntry::IndexedEntry {
                idx,
                entry: match entry {
                    EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.to_static()),
                    EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info),
                },
            },
            LogEntry::Expunged { end_size, end_hash } => LogEntry::Expunged { end_size, end_hash },

            LogEntry::Signature { size, signature } => LogEntry::Signature { size, signature },
        }
    }
}
