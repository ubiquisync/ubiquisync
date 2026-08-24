use crate::log_entry::{EntryBody, GenericLogEntry, OpBatch, OpOrExpunge, ToStatic};

impl<O: std::fmt::Debug, H: std::fmt::Debug> OpBatch<O, H> {
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, E, F, G>(
        &self,
        entry_idx: u64,
        transform_header: G,
        transform_op: F,
    ) -> Result<OpBatch<O2, H2>, E>
    where
        F: Fn(u64, u64, &O, &H2) -> Result<O2, E>,
        G: Fn(u64, &H) -> Result<H2, E>,
    {
        let header = transform_header(entry_idx, &self.header)?;
        let mut ops = vec![];
        for (idx, op) in self.ops.iter().enumerate() {
            ops.push(match op {
                OpOrExpunge::Op(op) => {
                    OpOrExpunge::Op(transform_op(entry_idx, idx as u64, op, &header)?)
                }
                OpOrExpunge::Expunge(hash) => OpOrExpunge::Expunge(*hash),
            })
        }
        Ok(OpBatch { header, ops })
    }
}

impl<O: std::fmt::Debug, H: std::fmt::Debug> GenericLogEntry<O, H> {
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, E, F, G>(
        &self,
        transform_header: G,
        transform_op: F,
    ) -> Result<GenericLogEntry<O2, H2>, E>
    where
        F: Fn(u64, u64, &O, &H2) -> Result<O2, E>,
        G: Fn(u64, &H) -> Result<H2, E>,
    {
        Ok(match self {
            GenericLogEntry::IndexedEntry { idx, entry } => GenericLogEntry::IndexedEntry {
                idx: *idx,
                entry: match entry {
                    EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.transform(
                        *idx,
                        transform_header,
                        transform_op,
                    )?),
                    EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info.clone()),
                },
            },
            GenericLogEntry::Expunged { range, cover } => GenericLogEntry::Expunged {
                range: range.clone(),
                cover: cover.clone(),
            },
            GenericLogEntry::Signature { size, signature } => GenericLogEntry::Signature {
                size: *size,
                signature: *signature,
            },
            GenericLogEntry::Unknown(unknown_entry_type) => {
                GenericLogEntry::Unknown(unknown_entry_type.clone())
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
            GenericLogEntry::Expunged { range, cover } => {
                GenericLogEntry::Expunged { range, cover }
            }

            GenericLogEntry::Signature { size, signature } => {
                GenericLogEntry::Signature { size, signature }
            }
            GenericLogEntry::Unknown(unknown_entry_type) => {
                GenericLogEntry::Unknown(unknown_entry_type)
            }
        }
    }
}
