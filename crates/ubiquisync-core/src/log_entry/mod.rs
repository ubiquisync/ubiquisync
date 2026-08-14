mod bytes;
mod error;
mod header;
mod ops;

pub use bytes::*;
pub use error::*;
pub use header::*;
pub use ops::*;

use crate::crypto::{CipherSuite, Hash, Signature};

/// One decoded entry: a live log entry or an expunged-entry marker.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum GenericLogEntry<Op: std::fmt::Debug, H: std::fmt::Debug> {
    IndexedEntry {
        idx: u64,
        entry: EntryBody<Op, H>,
    },
    Expunged {
        /// Inclusive start index.
        start_idx: u64,
        /// Inclusive end index.
        end_idx: u64,
        cover: Vec<Hash>,
    },
    Signature {
        size: u64,
        signature: Signature,
    },
    SealBranch {
        signature: Signature,
        start: EntryRef,
        end: EntryRef,
        ack_until: Option<EntryRef>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct EntryRef {
    pub hash: Hash,
    pub index: u64,
}

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = GenericLogEntry<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type OpaqueOpBatch<'a> = OpBatch<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = GenericLogEntry<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type PlaintextOpBatch<'a> = OpBatch<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type LogEntry<Op> = GenericLogEntry<Op, OpHeader>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EntryBody<Op: std::fmt::Debug, H: std::fmt::Debug> {
    OpBatch(OpBatch<Op, H>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    /// MUST NOT be expunged.
    UseKey(CipherInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct CipherInfo {
    pub cipher_suite: CipherSuite,
    pub fingerprint: Hash,
}

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
