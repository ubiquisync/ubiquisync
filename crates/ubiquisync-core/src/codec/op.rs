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
/// triple for the SQL op-log. `key` is the indexable identity (such as table + primary key).
/// `value` is the op's remaining payload.
///
/// # Round-trip
///
/// [`from_index_parts`](IndexableOp::from_index_parts) must invert
/// [`to_index_entry`](IndexableOp::to_index_entry), so the full op can be
/// reconstructed from its stored parts.
pub trait IndexableOp: Op {
    /// Split `self` into its `(tag, key, value)` triple.
    fn to_index_entry(&self) -> Result<OpIndexEntry, CodecError>;
    /// Reconstruct an op from a stored triple. The inverse of
    /// [`to_index_entry`](IndexableOp::to_index_entry).
    fn from_index_parts(tag: u8, key: &[u8], value: &[u8]) -> Result<Self, CodecError>;
}

/// The indexable form of an op: its tag plus the `key`/`value` split of its
/// body. See [`IndexableOp`].
pub struct OpIndexEntry {
    /// The op's tag byte (the same one [`Op::encode`] writes).
    pub tag: u8,
    /// Key part of op's for efficient querying based on affected row/entity.
    pub key: Vec<u8>,
    /// The op-specific payload after `key`; may be empty.
    pub value: Vec<u8>,
}
