use std::io::BufRead;

use crate::codec::{error::CodecError, reader::EntryBufferReader, writer::EntryBufferWriter};

/// An op vocabulary that can be encoded to / decoded from the log wire format.
///
/// Each data domain implements this for its own op type (e.g. the table op
/// enum in `ubiquisync-tables`, which is also named `Op`). The generic
/// [`Encoder`](crate::codec::Encoder) and [`Decoder`](crate::codec::Decoder)
/// drive it: the decoder reads the entry tag and hands it to [`decode`], the
/// encoder calls [`encode`] to write the op body. The codec framing supplies
/// everything else (timestamp, attribution, integrity hash).
///
/// [`decode`]: Op::decode
/// [`encode`]: Op::encode
pub trait Op: Sized {
    fn decode<R: BufRead>(tag: u8, r: &mut EntryBufferReader<R>) -> Result<Self, CodecError>;
    fn encode(&self, w: &mut EntryBufferWriter) -> Result<(), CodecError>;
}
