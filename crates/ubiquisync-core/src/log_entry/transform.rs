use crate::log_entry::{EntryBody, GenericLogEntry, OpBatch, OpOrExpunge};

impl<O: std::fmt::Debug, H: std::fmt::Debug> OpBatch<O, H> {
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, E, F, G>(
        &self,
        f: F,
        g: G,
    ) -> Result<OpBatch<O2, H2>, E>
    where
        F: Fn(&O, &H2) -> Result<O2, E>,
        G: Fn(&H) -> Result<H2, E>,
    {
        let header = g(&self.header)?;
        let mut ops = vec![];
        for op in self.ops.iter() {
            ops.push(match op {
                OpOrExpunge::Op(op) => OpOrExpunge::Op(f(op, &header)?),
                OpOrExpunge::Expunge(hash) => OpOrExpunge::Expunge(*hash),
            })
        }
        Ok(OpBatch { header, ops })
    }
}

impl<O: std::fmt::Debug, H: std::fmt::Debug> GenericLogEntry<O, H> {
    pub fn transform<O2: std::fmt::Debug, H2: std::fmt::Debug, E, F, G>(
        &self,
        f: F,
        g: G,
    ) -> Result<GenericLogEntry<O2, H2>, E>
    where
        F: Fn(&O, &H2) -> Result<O2, E>,
        G: Fn(&H) -> Result<H2, E>,
    {
        Ok(match self {
            GenericLogEntry::IndexedEntry { idx, entry } => GenericLogEntry::IndexedEntry {
                idx: *idx,
                entry: match entry {
                    EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.transform(f, g)?),
                    EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info.clone()),
                },
            },
            GenericLogEntry::Expunged {
                start_idx,
                end_idx,
                cover,
            } => GenericLogEntry::Expunged {
                start_idx: *start_idx,
                end_idx: *end_idx,
                cover: cover.clone(),
            },
            GenericLogEntry::Signature { size, signature } => GenericLogEntry::Signature {
                size: *size,
                signature: *signature,
            },
            GenericLogEntry::SealBranch {
                signature,
                start,
                end,
                ack_until,
            } => GenericLogEntry::SealBranch {
                signature: *signature,
                start: *start,
                end: *end,
                ack_until: *ack_until,
            },
        })
    }
}
