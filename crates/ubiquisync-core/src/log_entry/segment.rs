use std::{borrow::Borrow, ops::Range};

use thiserror::Error;

use crate::{
    codec::{reader::Reader, writer::Writer},
    log_entry::{
        CipherInfo, GenericLogEntry, LogDecodeError, LogEncodeError, PlaintextBytes,
        PlaintextLogEntry, ToStatic,
    },
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

pub fn decode_entries<'a, E, H>(
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

pub fn encode_entries<'a, E, H>(
    entries: impl Iterator<Item = GenericLogEntry<E, H>>,
) -> Result<Vec<u8>, LogEncodeError>
where
    E: Borrow<[u8]> + std::fmt::Debug,
    H: Borrow<[u8]> + std::fmt::Debug,
{
    let mut writer = Writer::new();
    for e in entries {
        e.encode(&mut writer)?;
    }
    Ok(writer.finalize())
}

#[derive(Error, Debug)]
pub enum SegmentEncodeError {
    #[error("entry encode error: {0}")]
    EntryEncodeError(#[from] LogEncodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum SegmentDecodeError {
    #[error("entry decode error: {0}")]
    EntryDecodeError(#[from] LogDecodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let buf = encode_entries(entries)?;
    Ok(zstd::encode_all(buf.as_slice(), 0)?)
}

fn decode_decompress_entries(
    range: &Range<u64>,
    buf: &[u8],
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let buf = zstd::decode_all(buf)?;
    let it = decode_entries::<PlaintextBytes, PlaintextBytes>(range, buf.as_slice());
    let mut res = vec![];
    for e in it {
        res.push(e?.to_static());
    }
    Ok(res)
}
