use std::ops::Range;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{CipherError, CipherInfo, CryptoDecodeError, Hash256, SegmentCipher, Signature},
    log::{ChainHash, LogDecodeError, LogEncodeError, LogEntry, OpaqueLogEntry, PlaintextLogEntry},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub range: Range<u64>,
    pub signature: Signature,
    pub prev_chain_hash: Hash256,
    pub encoding: SegmentEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentEncoding {
    Opaque,
    Plaintext(PlaintextSegmentEncoding),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaintextSegmentEncoding {
    pub outer_encryption: Option<EncryptionInfo>,
    pub inner_compression: Compression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionInfo {
    pub cipher: CipherInfo,
    pub nonce: Vec<u8>,
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

    pub fn read(
        self,
        cipher: &Option<SegmentCipher>,
    ) -> Result<DecodedSegment<'a>, SegmentDecodeError> {
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
    prev_chain: &ChainHash,
    entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    if prev_chain.size != range.start {
        return Err(SegmentEncodeError::RangeMismatch {
            actual: range.clone(),
            expected: prev_chain.size..range.end,
        });
    }
    let mut w = Writer::new();
    let header = SegmentHeader {
        range: range.clone(),
        signature: *signature,
        prev_chain_hash: prev_chain.hash,
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
    prev_chain: &ChainHash,
    cipher: &Option<SegmentCipher>,
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    if prev_chain.size != range.start {
        return Err(SegmentEncodeError::RangeMismatch {
            actual: range.clone(),
            expected: prev_chain.size..range.end,
        });
    }
    let mut w = Writer::new();
    let (outer_encryption, nonce) = if let Some(cipher) = cipher {
        let nonce_size = cipher.cipher_suite().nonce_size();
        let mut nonce = vec![0; nonce_size];
        getrandom::fill(nonce.as_mut_slice())
            .map_err(|_| SegmentEncodeError::NonceGenerationError)?;
        (
            Some(EncryptionInfo {
                cipher: cipher.cipher_info(),
                nonce: nonce.clone(),
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
        prev_chain_hash: prev_chain.hash,
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

pub fn decode_entries<'a, E>(
    start: u64,
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<LogEntry<E>, LogDecodeError>>
where
    E: From<&'a [u8]> + BytesWrapper,
{
    let mut reader = Reader::new(bytes);
    let mut next_entry_index = start;
    std::iter::from_fn(move || {
        if reader.is_empty() {
            None
        } else {
            let e = LogEntry::decode(&mut reader, next_entry_index);
            if let Ok(ref e) = e
                && let Some(idx) = e.entry_index()
            {
                if let Some(next) = idx.checked_add(1) {
                    // NOTE: indexes aren't wire controlled, this is purely defensive
                    next_entry_index = next
                } else {
                    return Some(Err(LogDecodeError::U64AddOverflow(idx, 1)));
                }
            }
            Some(e)
        }
    })
}

pub fn encode_entries<'a, E>(
    entries: impl Iterator<Item = &'a LogEntry<E>>,
    writer: &mut Writer,
) -> Result<Range<u64>, LogEncodeError>
where
    E: BytesWrapper + 'a,
{
    let mut have_start = false;
    let mut start = 0;
    let mut end = 0;
    for e in entries {
        if let Some(idx) = e.entry_index() {
            if !have_start {
                start = idx;
                have_start = true;
            }
            end = idx + 1;
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
    segment_cipher: &SegmentCipher,
    nonce: &[u8],
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let (mut inout, range) = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(&range, nonce, &mut inout)?;
    Ok((inout, range))
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, Range<u64>), SegmentEncodeError> {
    let mut w = Writer::new();
    let r = encode_entries(entries, &mut w)?;
    Ok((zstd::encode_all(w.finalize().as_slice(), 0)?, r))
}

/// 256mb decode limit
const ZSTD_DECODE_LIMIT: usize = 1usize << 28;

fn decompress_decode_entries(
    start: u64,
    buf: &[u8],
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    // TODO: decode with limit
    // let mut buf = vec![];
    // let buf = zstd::Decoder::new(buf)?
    //     .take(ZSTD_DECODE_LIMIT)
    //     .read_to_end(&mut buf);
    let buf = zstd::decode_all(buf)?;
    let it = decode_entries::<PlaintextBytes>(start, buf.as_slice());
    let mut res = vec![];
    for e in it {
        res.push(e?.to_static());
    }
    Ok(res)
}

fn decrypt_decompress_decode_entries(
    segment_cipher: &SegmentCipher,
    range: &Range<u64>,
    nonce: &[u8],
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
        w.write_len_prefixed(&self.nonce);
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let cipher = CipherInfo::decode(r)?;
        let nonce = r.read_len_prefixed()?;
        Ok(Self {
            cipher,
            nonce: nonce.into(),
        })
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

/// Computes the index range of the segment. Returns 0..0 if the segment has no indexed entries.
pub fn compute_segment_range<'a, B: std::fmt::Debug + 'a>(
    entries: impl Iterator<Item = &'a LogEntry<B>>,
) -> Range<u64> {
    let mut have_start = false;
    let mut start = 0;
    let mut end = 0;
    for entry in entries {
        if let Some(idx) = entry.entry_index() {
            if !have_start {
                start = idx;
                have_start = true;
            }
            end = idx + 1;
        }
    }
    start..end
}

#[cfg(test)]
pub(crate) mod tests {
    use proptest::strategy::{BoxedStrategy, Strategy};
    #[cfg(test)]
    use secrecy::SecretBox;
    use test_strategy::{Arbitrary, proptest};

    use crate::{
        bytes::PlaintextBytes,
        crypto::{RootKey256, Signature},
        ids::LogId,
        log::{
            ChainHash, EntryBody, LogEntry, OpBatch, PlaintextLogEntry,
            segment::{
                SegmentEncoding, compute_segment_range, encode_segment_opaque,
                encode_segment_plaintext,
            },
            segment_to_opaque,
        },
    };
    #[cfg(test)]
    use crate::{
        crypto::{SegmentCipher, SegmentCipherSuite},
        log::{ChainSeed, segment::SegmentReader},
    };

    #[derive(Debug)]
    pub(crate) struct LogEntries {
        pub start_index: u64,
        pub entries: Vec<PlaintextLogEntry<'static>>,
    }

    impl proptest::arbitrary::Arbitrary for LogEntries {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            (
                (0u64..1 << 24),
                proptest::collection::vec(proptest::arbitrary::any::<GeneratedEntry>(), 1..=256),
            )
                .prop_map(|(start_index, entries)| LogEntries {
                    start_index,
                    entries: GeneratedEntry::to_entries(entries, start_index).collect(),
                })
                .boxed()
        }
    }

    #[derive(Arbitrary, Debug)]
    pub(crate) enum GeneratedEntry {
        Ops(OpBatch<PlaintextBytes<'static>>),
        Signature(Signature),
    }

    impl GeneratedEntry {
        pub(crate) fn to_entries(
            entries: Vec<Self>,
            mut next_entry_index: u64,
        ) -> impl Iterator<Item = PlaintextLogEntry<'static>> {
            entries.into_iter().map(move |e| match e {
                GeneratedEntry::Ops(ops) => {
                    let e = LogEntry::IndexedEntry {
                        idx: next_entry_index,
                        entry: EntryBody::OpBatch(ops),
                    };
                    next_entry_index += 1;
                    e
                }
                GeneratedEntry::Signature(signature) => LogEntry::Signature {
                    size: next_entry_index,
                    signature,
                },
            })
        }
    }

    #[proptest(cases = 10)]
    fn test_segments_no_cipher(entries: LogEntries, log_id: LogId) {
        let range = compute_segment_range(entries.entries.iter());
        assert_eq!(range.start, entries.start_index);
        let seed = ChainSeed::new(&log_id);
        let start_chain = ChainHash {
            size: range.start,
            hash: [3; 32],
        };
        let sig = Signature::Ed25519([2; 64]);

        // test opaque encoding
        {
            let (opaque, _) =
                segment_to_opaque(&None, entries.entries.iter(), &seed, &start_chain).unwrap();
            let segment = encode_segment_opaque(&range, &sig, &start_chain, opaque.iter()).unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(range, header.range);
            assert_eq!(start_chain.hash, header.prev_chain_hash);
            assert_eq!(header.encoding, SegmentEncoding::Opaque);
            let decoded = reader.read(&None).unwrap();
            match decoded {
                crate::log::segment::DecodedSegment::Opaque(items) => {
                    assert_eq!(opaque, items);
                }
                crate::log::segment::DecodedSegment::Plaintext(_) => unreachable!(),
            }
        }

        // test plaintext encoding (basically just compression)
        {
            let segment =
                encode_segment_plaintext(&range, &sig, &start_chain, &None, entries.entries.iter())
                    .unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(range, header.range);
            assert_eq!(start_chain.hash, header.prev_chain_hash);
            let decoded = reader.read(&None).unwrap();
            match decoded {
                crate::log::segment::DecodedSegment::Plaintext(items) => {
                    assert_eq!(entries.entries, items);
                }
                crate::log::segment::DecodedSegment::Opaque(_) => unreachable!(),
            }
        }
    }

    #[proptest(cases = 10)]
    fn test_segments_with_cipher(entries: LogEntries, log_id: LogId, key: [u8; 32]) {
        let key = RootKey256::new(SecretBox::new(Box::new(key)));
        let cipher = Some(SegmentCipher::new(
            SegmentCipherSuite::XChaCha20Poly1305,
            key.container_key(&log_id.container_id),
            &log_id,
        ));
        let range = compute_segment_range(entries.entries.iter());
        assert_eq!(range.start, entries.start_index);
        let chain_start = ChainHash {
            size: range.start,
            hash: [3; 32],
        };
        let sig = Signature::Ed25519([2; 64]);

        let segment =
            encode_segment_plaintext(&range, &sig, &chain_start, &cipher, entries.entries.iter())
                .unwrap();
        let reader = SegmentReader::start(&segment).unwrap();
        let header = reader.header();
        assert_eq!(sig, header.signature);
        assert_eq!(range, header.range);
        assert_eq!(chain_start.hash, header.prev_chain_hash);
        let decoded = reader.read(&cipher).unwrap();
        match decoded {
            crate::log::segment::DecodedSegment::Plaintext(items) => {
                assert_eq!(entries.entries, items);
            }
            crate::log::segment::DecodedSegment::Opaque(_) => unreachable!(),
        }
    }
}
