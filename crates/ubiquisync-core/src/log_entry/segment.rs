use std::{borrow::Borrow, ops::Range};

use thiserror::Error;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::{CipherError, SegmentCipher},
    ids::LogId,
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
    pub nonce: [u8; 16],
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
    start: u64,
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<GenericLogEntry<E, H>, LogDecodeError>>
where
    E: From<&'a [u8]> + std::fmt::Debug,
    H: From<&'a [u8]> + std::fmt::Debug,
{
    let mut reader = Reader::new(bytes);
    let mut next_entry_index = start;
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
) -> Result<(Vec<u8>, Range<u64>), LogEncodeError>
where
    E: Borrow<[u8]> + std::fmt::Debug,
    H: Borrow<[u8]> + std::fmt::Debug,
{
    let mut writer = Writer::new();
    let mut start = 0;
    let mut end = 0;
    for e in entries {
        if let Some(idx) = e.next_entry_index() {
            if start == 0 {
                if idx > 0 {
                    start = idx - 1;
                }
            }
            end = idx;
        }
        e.encode(&mut writer)?;
    }
    Ok((writer.finalize(), start..end))
}

#[derive(Error, Debug)]
pub enum SegmentEncodeError {
    #[error("entry encode error: {0}")]
    EntryEncodeError(#[from] LogEncodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("cipher error: {0}")]
    CipherError(#[from] CipherError),
}

#[derive(Error, Debug)]
pub enum SegmentDecodeError {
    #[error("entry decode error: {0}")]
    EntryDecodeError(#[from] LogDecodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("cipher error: {0}")]
    CipherError(#[from] CipherError),
}

fn encode_compress_encrypt_entries<'a>(
    segment_cipher: &SegmentCipher,
    log_id: &LogId,
    nonce: [u8; 16],
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let (mut inout, range) = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(log_id, &range, nonce, &mut inout)?;
    Ok((inout, range))
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let (buf, r) = encode_entries(entries)?;
    Ok((zstd::encode_all(buf.as_slice(), 0)?, r))
}

fn decompress_decode_entries(
    start: u64,
    buf: &[u8],
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let buf = zstd::decode_all(buf)?;
    let it = decode_entries::<PlaintextBytes, PlaintextBytes>(start, buf.as_slice());
    let mut res = vec![];
    for e in it {
        res.push(e?.to_static());
    }
    Ok(res)
}

fn decrypt_decompress_decode_entries(
    segment_cipher: &SegmentCipher,
    range: &Range<u64>,
    buf: &mut Vec<u8>,
    log_id: &LogId,
    nonce: [u8; 16],
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    segment_cipher.decrypt_segment(log_id, range, nonce, buf)?;
    decompress_decode_entries(range.start, buf.as_slice())
}
