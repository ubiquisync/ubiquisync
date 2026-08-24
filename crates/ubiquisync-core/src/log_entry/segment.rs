use std::ops::Range;

use crate::{
    codec::reader::Reader,
    log_entry::{CipherInfo, GenericLogEntry, LogDecodeError, OpaqueLogEntry, PlaintextLogEntry},
};

pub enum SegmentEncoding {
    Opaque,
    Plaintext(PlaintextSegmentEncoding),
}

pub struct PlaintextSegmentEncoding {
    pub outer_encryption: Option<CipherInfo>,
    pub inner_compression: Compression,
}

pub struct EncryptionInfo {
    pub cipher: CipherInfo,
    pub nonce: Vec<u8>,
}

pub enum Compression {
    Zstd = 0,
}

// pub enum DecodedSegment<'a> {
//     OpaqueSegment(Vec<OpaqueLogEntry<'a>>)
//     PlaintextSegment(Vec<PlaintextLogEntry<'a>>)
// }

// use std::ops::Range;

// use crate::{
//     codec::reader::Reader,
//     init::Version,
//     log_entry::{CipherInfo, GenericLogEntry, LogDecodeError, OpaqueLogEntry},
// };

// pub struct SegmentMeta {
//     pub range: Range<u64>,
// }

// pub struct SegmentHeader {
//     pub version: Version,
//     pub encoding: SegmentEncoding,
// }

// pub enum SegmentDecodeError {}

pub fn decode_segment<'a, E, H>(
    range: &Range<u64>,
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<GenericLogEntry<E, H>, LogDecodeError>>
where
    E: From<&'a [u8]> + std::fmt::Debug,
    H: From<&'a [u8]> + std::fmt::Debug,
{
    let mut reader = Reader::new(bytes);
    let mut next_entry_index = range.start;
    std::iter::from_fn(move || {
        if reader.is_empty() {
            None
        } else {
            let e = GenericLogEntry::decode(&mut reader, next_entry_index);
            if let Ok(ref e) = e {
                if let Some(idx) = e.next_entry_index() {
                    next_entry_index = idx;
                }
            }
            Some(e)
        }
    })
}
