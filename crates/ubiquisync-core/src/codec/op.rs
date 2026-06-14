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
