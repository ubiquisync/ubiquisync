//! Wire-level log entry: a single op with timestamp and optional
//! server-attested user attribution. This is the unit of encoding/decoding in a
//! segment file.

use std::borrow::Cow;

use crate::crypto::Signature;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueBytes<'a>(pub Cow<'a, [u8]>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaintextBytes<'a>(pub Cow<'a, [u8]>);

/// Log entry where op and header are encoded as canonical hash bytes (may be encrypted)
pub type OpaqueLogEntry<'a> = GenericLogEntry<OpaqueBytes<'a>, OpaqueBytes<'a>>;

pub type PlaintextLogEntry<'a> = GenericLogEntry<PlaintextBytes<'a>, PlaintextBytes<'a>>;

pub type LogEntry<Op> = GenericLogEntry<Op, OpHeader>;

#[derive(Clone)]
pub enum EntryBody<Op, H> {
    OpBatch(OpBatch<Op, H>),
    /// Declares the fingerprint for the encryption key being used from
    /// this point forward until the next UseKey op changes the key.
    /// MUST NOT be expunged.
    UseKey(Hash),
}

pub type Hash = [u8; 32];
