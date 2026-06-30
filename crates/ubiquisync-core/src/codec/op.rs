use std::io::BufRead;

use crate::codec::{error::CodecError, reader::EntryBufferReader, writer::EntryBufferWriter};

/// An op vocabulary that can be encoded to / decoded from the log wire format.
///
/// Each data domain implements this for its own op type (e.g. the table op
/// enum in `ubiquisync-tables`, which is also named `Op`). The generic
/// [`Encoder`](crate::codec::Encoder) and [`Decoder`](crate::codec::Decoder)
/// drive it: the framing reads the entry tag and hands it to [`decode`], which
/// decodes the body; [`encode`] is responsible for the **whole** op — it writes
/// the tag *and* the body. The framing supplies everything else (timestamp,
/// attribution, integrity hash).
///
/// # Reserved tag
///
/// Tag `255` (`0xFF`, [`TAG_EXPUNGED`](crate::codec::TAG_EXPUNGED)) is reserved
/// by the framing for expunged-entry markers and is **not a valid op tag**. An
/// implementation must never write `0xFF` as its tag, and [`decode`] is never
/// called with `tag == 0xFF` — the framing intercepts that value before
/// dispatching. Emitting `0xFF` from [`encode`] would make the entry decode as
/// an expunged marker, silently corrupting the log.
///
/// [`decode`]: Op::decode
/// [`encode`]: Op::encode
pub trait Op: Sized {
    /// Decode an op body for the given entry `tag`, which the framing has
    /// already read. `tag` is never `0xFF`
    /// ([`TAG_EXPUNGED`](crate::codec::TAG_EXPUNGED)) — the framing handles that
    /// reserved value itself.
    fn decode<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError>;
    /// Encode the op as a tag byte followed by its body. The tag must never be
    /// `0xFF` ([`TAG_EXPUNGED`](crate::codec::TAG_EXPUNGED)), which is reserved.
    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError>;
}

/// An [`Op`] that can also be split into an indexable `(tag, key, value)`
/// triple for the SQL op-log, where `key` carries row identity (table id +
/// primary key) so history can be queried per-row or per-table.
///
/// # Round-trip and cross-consistency
///
/// [`from_index_parts`](IndexableOp::from_index_parts) must invert
/// [`to_index_entry`](IndexableOp::to_index_entry). Critically, the split must
/// also agree with [`Op::encode`]: `key` followed by `value` must reproduce
/// exactly the body [`Op::encode`] writes after the tag. Entries are hashed
/// over that canonical byte form in both the folder log and the SQL op-log, so
/// any divergence would give the *same* entry two different hashes depending on
/// its source — breaking expunged markers (`tag = 0xFF`, value = the hash)
/// across peers.
///
/// # Key encoding
///
/// `key` must place the table id as a fixed-width leading prefix, so the single
/// op-log key column serves both exact-row lookups (`key = ?`) and whole-table
/// scans (a prefix range).
pub trait IndexableOp: Op {
    /// Split `self` into its `(tag, key, value)` triple. `key ++ value` must
    /// equal the body [`Op::encode`] writes after the tag.
    fn to_index_entry(&self) -> Result<OpIndexEntry, CodecError>;
    /// Reconstruct an op from a stored triple. The inverse of
    /// [`to_index_entry`](IndexableOp::to_index_entry).
    fn from_index_parts(tag: u8, key: &[u8], value: &[u8]) -> Result<Self, CodecError>;
}

/// The indexable form of an op: a tag plus the identity (`key`) and payload
/// (`value`) halves of its encoded body. See [`IndexableOp`].
pub struct OpIndexEntry {
    /// Op variant discriminant. Never `0xFF`, which is reserved for expunged
    /// markers.
    pub tag: u8,
    /// Row identity — table id (fixed-width prefix) followed by the primary key.
    pub key: Vec<u8>,
    /// The remainder of the op body after `key`.
    pub value: Vec<u8>,
}
