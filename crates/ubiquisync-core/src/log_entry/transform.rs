use crate::{
    crypto::Hash256,
    log_entry::{EntryBody, GenericLogEntry, OpBatch, OpOrExpunge, ToStatic},
};

impl<O: std::fmt::Debug, H: std::fmt::Debug> OpBatch<O, H> {
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

impl<O: std::fmt::Debug, H: std::fmt::Debug> GenericLogEntry<O, H> {
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, A, B, C, D, S, E>(
        &self,
        init_state: A,
        transform_header: B,
        transform_op: C,
        on_expunge_op: D,
    ) -> Result<(GenericLogEntry<O2, H2>, Option<S>), E>
    where
        A: Fn(u64, &OpBatch<O, H>) -> Result<S, E>,
        B: Fn(&H, &mut S) -> Result<H2, E>,
        C: Fn(u64, &O, &mut S) -> Result<O2, E>,
        D: Fn(u64, &Hash256, &mut S) -> Result<(), E>,
    {
        Ok(match self {
            GenericLogEntry::IndexedEntry { idx, entry } => match entry {
                EntryBody::OpBatch(op_batch) => {
                    let (op_batch2, state) = op_batch.transform(
                        *idx,
                        init_state,
                        transform_header,
                        transform_op,
                        on_expunge_op,
                    )?;
                    (
                        GenericLogEntry::IndexedEntry {
                            entry: EntryBody::OpBatch(op_batch2),
                            idx: *idx,
                        },
                        Some(state),
                    )
                }
                EntryBody::UseKey(cipher_info) => (
                    GenericLogEntry::IndexedEntry {
                        entry: EntryBody::UseKey(cipher_info.clone()),
                        idx: *idx,
                    },
                    None,
                ),
            },
            GenericLogEntry::Expunged {
                range,
                cover,
                last_leaf_hash,
            } => (
                GenericLogEntry::Expunged {
                    range: range.clone(),
                    cover: cover.clone(),
                    last_leaf_hash: *last_leaf_hash,
                },
                None,
            ),
            GenericLogEntry::Signature { size, signature } => (
                GenericLogEntry::Signature {
                    size: *size,
                    signature: *signature,
                },
                None,
            ),
            GenericLogEntry::Unknown(unknown_entry_type) => {
                (GenericLogEntry::Unknown(unknown_entry_type.clone()), None)
            }
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

impl<O: ToStatic + std::fmt::Debug, H: ToStatic + std::fmt::Debug> ToStatic
    for GenericLogEntry<O, H>
where
    O::Static: std::fmt::Debug,
    H::Static: std::fmt::Debug,
{
    type Static = GenericLogEntry<O::Static, H::Static>;

    fn to_static(self) -> Self::Static {
        match self {
            GenericLogEntry::IndexedEntry { idx, entry } => GenericLogEntry::IndexedEntry {
                idx,
                entry: match entry {
                    EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.to_static()),
                    EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info),
                },
            },
            GenericLogEntry::Expunged {
                range,
                cover,
                last_leaf_hash,
            } => GenericLogEntry::Expunged {
                range,
                cover,
                last_leaf_hash,
            },

            GenericLogEntry::Signature { size, signature } => {
                GenericLogEntry::Signature { size, signature }
            }
            GenericLogEntry::Unknown(unknown_entry_type) => {
                GenericLogEntry::Unknown(unknown_entry_type)
            }
        }
    }
}
