use std::{borrow::Borrow, ops::Range};

use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{Cipher, CipherError, CipherInfo, CryptoDecodeError, Hash256, Signature},
    log::{ChainHash, LogDecodeError, LogEncodeError, LogEntry, OpaqueLogEntry, PlaintextLogEntry},
};

pub struct SegmentDescriptor {
    pub start: u64,
    pub root_info: ChainHash,
}

pub struct SegmentHeader {
    pub range: Range<u64>,
    pub signature: Signature,
    pub prev_chain_hash: Hash256,
    pub encoding: SegmentEncoding,
}

pub enum SegmentEncoding {
    Opaque,
    Plaintext(PlaintextSegmentEncoding),
}

pub struct PlaintextSegmentEncoding {
    pub outer_encryption: Option<EncryptionInfo>,
    pub inner_compression: Compression,
}

pub struct EncryptionInfo {
    pub cipher: CipherInfo,
    pub nonce: [u8; 16],
}

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Compression {
    Zstd = 0,
}

pub enum DecodedSegment<'a> {
    Opaque(Vec<OpaqueLogEntry<'a>>),
    Plaintext(Vec<PlaintextLogEntry<'a>>),
}

pub struct SegmentReader<'a> {
    reader: Reader<'a>,
    header: SegmentHeader,
}

impl<'a> SegmentReader<'a> {
    pub fn start(buf: &'a [u8]) -> Result<Self, SegmentDecodeError> {
        let mut reader = Reader::new(buf);
        let header = SegmentHeader::decode(&mut reader)?;
        Ok(Self { reader, header })
    }

    pub fn header(&self) -> &SegmentHeader {
        &self.header
    }

    pub fn read(self, cipher: Option<&Cipher>) -> Result<DecodedSegment<'a>, SegmentDecodeError> {
        let start = self.header.range.start;
        let buf = self.reader.into_remaining();
        Ok(match self.header.encoding {
            SegmentEncoding::Opaque => {
                let mut res = vec![];
                for e in decode_entries(start, buf) {
                    res.push(e?);
                }
                DecodedSegment::Opaque(res)
            }
            SegmentEncoding::Plaintext(enc) => {
                DecodedSegment::Plaintext(match enc.outer_encryption {
                    Some(c) => {
                        if let Some(cipher) = cipher
                            && c.cipher == cipher.cipher_info()
                        {
                            decrypt_decompress_decode_entries(
                                cipher,
                                &self.header.range,
                                &c.nonce,
                                buf,
                            )?
                        } else {
                            return Err(SegmentDecodeError::MissingSegmentCipher(c.cipher));
                        }
                    }
                    None => decompress_decode_entries(start, buf)?,
                })
            }
        })
    }
}

pub fn encode_segment_opaque<'a>(
    range: &Range<u64>,
    signature: &Signature,
    prev_chain_hash: &Hash256,
    entries: impl Iterator<Item = OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let header = SegmentHeader {
        range: range.clone(),
        signature: *signature,
        prev_chain_hash: *prev_chain_hash,
        encoding: SegmentEncoding::Opaque,
    };
    header.encode(&mut w)?;
    let range2 = encode_entries(entries, &mut w)?;
    if range != &range2 {
        return Err(SegmentEncodeError::RangeMismatch {
            actual: range2,
            expected: range.clone(),
        });
    }
    Ok(w.finalize())
}

pub fn encode_segment_plaintext<'a>(
    range: &Range<u64>,
    signature: &Signature,
    prev_chain_hash: &Hash256,
    cipher: Option<&Cipher>,
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let (outer_encryption, nonce) = if let Some(cipher) = cipher {
        let mut nonce = [0; 16];
        // TODO verify this is a cryptographically secure RNG
        getrandom::fill(&mut nonce).map_err(|_| SegmentEncodeError::NonceGenerationError)?;
        (
            Some(EncryptionInfo {
                cipher: cipher.cipher_info(),
                nonce,
            }),
            Some(nonce),
        )
    } else {
        (None, None)
    };
    let encoding = PlaintextSegmentEncoding {
        outer_encryption,
        inner_compression: Compression::Zstd,
    };
    let header = SegmentHeader {
        range: range.clone(),
        signature: *signature,
        prev_chain_hash: *prev_chain_hash,
        encoding: SegmentEncoding::Plaintext(encoding),
    };
    header.encode(&mut w)?;
    let (buf, range2) = if let Some(cipher) = cipher {
        encode_compress_encrypt_entries(cipher, &nonce.unwrap(), entries)
    } else {
        encode_compress_entries(entries)
    }?;
    if range != &range2 {
        return Err(SegmentEncodeError::RangeMismatch {
            actual: range2,
            expected: range.clone(),
        });
    }
    w.write_slice(buf.as_slice());
    Ok(w.finalize())
}

pub fn decode_entries<'a, E, H>(
    start: u64,
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<LogEntry<E, H>, LogDecodeError>>
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
            let e = LogEntry::decode(&mut reader, next_entry_index);
            if let Ok(ref e) = e
                && let Some(idx) = e.next_entry_index()
            {
                next_entry_index = idx;
            }
            Some(e)
        }
    })
}

pub fn encode_entries<E, H>(
    entries: impl Iterator<Item = LogEntry<E, H>>,
    writer: &mut Writer,
) -> Result<Range<u64>, LogEncodeError>
where
    E: Borrow<[u8]> + std::fmt::Debug,
    H: Borrow<[u8]> + std::fmt::Debug,
{
    let mut start = 0;
    let mut end = 0;
    for e in entries {
        if let Some(idx) = e.next_entry_index() {
            if start == 0 && idx > 0 {
                start = idx - 1;
            }
            end = idx;
        }
        e.encode(writer)?;
    }
    Ok(start..end)
}

#[derive(Error, Debug)]
pub enum SegmentEncodeError {
    #[error("entry encode error: {0}")]
    EntryEncodeError(#[from] LogEncodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("cipher error: {0}")]
    CipherError(#[from] CipherError),
    #[error("write error: {0}")]
    WriteError(#[from] WriteError),

    #[error("range mismatch, expected {expected:?}, got {actual:?}")]
    RangeMismatch {
        actual: Range<u64>,
        expected: Range<u64>,
    },
    #[error("nonce generation error")]
    NonceGenerationError,
}

#[derive(Error, Debug)]
pub enum SegmentDecodeError {
    #[error("entry decode error: {0}")]
    LogDecodeError(#[from] LogDecodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("cipher error: {0}")]
    CipherError(#[from] CipherError),
    #[error("unknown signature algorithm: {0}")]
    UnknownSignatureAlgorithm(u8),
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
    #[error("unknown segment encoding: {0}")]
    UnknownSegmentEncoding(u8),
    #[error("unknown compression: {0}")]
    UnknownCompression(u8),
    #[error("unknown encryption info type: {0}")]
    UnknownEncryptionInfo(u8),
    #[error("missing segment cipher {0:?}")]
    MissingSegmentCipher(CipherInfo),
}

fn encode_compress_encrypt_entries<'a>(
    segment_cipher: &Cipher,
    nonce: &[u8; 16],
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let (mut inout, range) = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(&range, nonce, &mut inout)?;
    Ok((inout, range))
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let mut w = Writer::new();
    let r = encode_entries(entries, &mut w)?;
    Ok((zstd::encode_all(w.finalize().as_slice(), 0)?, r))
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
    segment_cipher: &Cipher,
    range: &Range<u64>,
    nonce: &[u8; 16],
    buf: &[u8], // TODO we could maybe use parent vec as inout buffer for less alloc in the future
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let mut buf = Vec::from(buf);
    segment_cipher.decrypt_segment(range, nonce, &mut buf)?;
    decompress_decode_entries(range.start, buf.as_slice())
}

impl SegmentHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_range(&self.range)?;
        self.signature.encode(w);
        w.write_array(&self.prev_chain_hash);
        self.encoding.encode(w);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let range = r.read_range()?;
        let signature = Signature::decode(r).map_err(SegmentDecodeError::from_sig_decode_err)?;
        let prev_chain_hash: Hash256 = r.read_array()?;
        let encoding = SegmentEncoding::decode(r)?;
        Ok(Self {
            range,
            signature,
            prev_chain_hash,
            encoding,
        })
    }
}

impl SegmentEncoding {
    pub fn encode(&self, w: &mut Writer) {
        match self {
            SegmentEncoding::Opaque => {
                w.write_byte(SEGMENT_ENCODING_OPAQUE);
            }
            SegmentEncoding::Plaintext(enc) => {
                w.write_byte(SEGMENT_ENCODING_PLAINTEXT);
                enc.encode(w);
            }
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        match r.read_byte()? {
            SEGMENT_ENCODING_OPAQUE => Ok(Self::Opaque),
            SEGMENT_ENCODING_PLAINTEXT => Ok(Self::Plaintext(PlaintextSegmentEncoding::decode(r)?)),
            b => Err(SegmentDecodeError::UnknownSegmentEncoding(b)),
        }
    }
}

impl PlaintextSegmentEncoding {
    pub fn encode(&self, w: &mut Writer) {
        if let Some(ref enc) = self.outer_encryption {
            w.write_byte(1);
            enc.encode(w);
        } else {
            w.write_byte(0);
        }
        w.write_byte(self.inner_compression.into());
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let outer_encryption = match r.read_byte()? {
            0 => None,
            1 => Some(EncryptionInfo::decode(r)?),
            b => return Err(SegmentDecodeError::UnknownEncryptionInfo(b)),
        };
        let inner_compression = Compression::try_from(r.read_byte()?)
            .map_err(|e| SegmentDecodeError::UnknownCompression(e.number))?;
        Ok(Self {
            outer_encryption,
            inner_compression,
        })
    }
}

impl EncryptionInfo {
    pub fn encode(&self, w: &mut Writer) {
        self.cipher.encode(w);
        w.write_array(&self.nonce);
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let cipher = CipherInfo::decode(r)?;
        let nonce = r.read_array()?;
        Ok(Self { cipher, nonce })
    }
}

impl SegmentDecodeError {
    fn from_sig_decode_err(err: CryptoDecodeError) -> Self {
        match err {
            CryptoDecodeError::ReadError(e) => SegmentDecodeError::ReadError(e),
            CryptoDecodeError::UnknownAlgorithm(b) => {
                SegmentDecodeError::UnknownSignatureAlgorithm(b)
            }
        }
    }
}

const SEGMENT_ENCODING_OPAQUE: u8 = 0;
const SEGMENT_ENCODING_PLAINTEXT: u8 = 1;
