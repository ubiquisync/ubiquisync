use std::borrow::{Borrow, Cow};

use crate::crypto::{CipherSuite, Hash, Signature};
use crate::hlc::Timestamp;
use crate::uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpBatch<Op, H = OpHeader> {
    pub header: H,
    pub ops: Vec<OpOrExpunge<Op>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpHeader {
    /// The **server-attested** user id for this entry. Every entry originates
    /// from *some* user, but this field specifically carries the identity a
    /// server vouched for — it is populated only in server-mode segments, where
    /// the server asserts attribution. `None` in device mode, where attribution
    /// is implicit from the peer directory and no server assertion exists.
    ///
    /// Do not read this as "the author"; read it as "who the server said this
    /// was." It is distinct from a stream's `peer_id` (which stream the entry
    /// came from).
    ///
    /// This _can_ be empty in server logs if and only if none of the ops are user attributable.
    pub server_user_id: Option<Uuid>,
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpOrExpunge<Op> {
    Op(Op),
    Expunge(Hash),
}

/// One decoded entry: a live log entry or an expunged-entry marker.
#[derive(Clone)]
pub enum GenericLogEntry<Op, H> {
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

#[derive(Clone, Copy)]
pub struct EntryRef {
    pub hash: Hash,
    pub index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> Borrow<[u8]> for OpaqueBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaintextBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> Borrow<[u8]> for PlaintextBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = GenericLogEntry<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type OpaqueOpBatch<'a> = OpBatch<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = GenericLogEntry<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type PlaintextOpBatch<'a> = OpBatch<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type LogEntry<Op> = GenericLogEntry<Op, OpHeader>;

#[derive(Clone)]
pub enum EntryBody<Op, H> {
    OpBatch(OpBatch<Op, H>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    /// MUST NOT be expunged.
    UseKey(CipherInfo),
}

#[derive(Clone)]
pub struct CipherInfo {
    pub cipher_suite: CipherSuite,
    pub fingerprint: Hash,
}

impl<O, H> OpBatch<O, H> {
    pub fn transform<O2, H2, E, F, G>(&self, f: F, g: G) -> Result<OpBatch<O2, H2>, E>
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

impl<O, H> GenericLogEntry<O, H> {
    pub fn transform<O2, H2, E, F, G>(&self, f: F, g: G) -> Result<GenericLogEntry<O2, H2>, E>
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
