use std::io::Read;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{CipherError, CipherInfo, CryptoDecodeError, SegmentCipher, Signature},
    log::{ChainHash, LogDecodeError, LogEncodeError, LogEntry, OpaqueLogEntry, PlaintextLogEntry},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub signature: Signature,
    pub prev_chain: ChainHash,
    pub count: u64,
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
        let buf = self.reader.into_remaining();
        Ok(match self.header.encoding {
            SegmentEncoding::Opaque => {
                let mut res = vec![];
                for e in decode_entries(buf) {
                    res.push(e?);
                }
                check_entry_count(res.iter(), self.header.count)?;
                DecodedSegment::Opaque(res)
            }
            SegmentEncoding::Plaintext(enc) => DecodedSegment::Plaintext({
                let entries = match enc.outer_encryption {
                    Some(c) => {
                        if let Some(cipher) = cipher
                            && c.cipher == cipher.cipher_info()
                        {
                            decrypt_decompress_decode_entries(
                                cipher,
                                &self.header.prev_chain,
                                self.header.count,
                                &c.nonce,
                                buf,
                            )?
                        } else {
                            return Err(SegmentDecodeError::MissingSegmentCipher(c.cipher));
                        }
                    }
                    None => decompress_decode_entries(buf)?,
                };
                check_entry_count(entries.iter(), self.header.count)?;
                entries
            }),
        })
    }
}

pub fn encode_segment_opaque<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    count: u64,
    entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let header = SegmentHeader {
        prev_chain: *prev_chain,
        signature: *signature,
        count,
        encoding: SegmentEncoding::Opaque,
    };
    header.encode(&mut w)?;
    let count2 = encode_entries(entries, &mut w)?;
    if count != count2 {
        return Err(SegmentEncodeError::CountMismatch {
            actual: count2,
            expected: count,
        });
    }
    Ok(w.finalize())
}

pub fn encode_segment_plaintext<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    cipher: &Option<SegmentCipher>,
    entries: &[PlaintextLogEntry<'a>],
) -> Result<Vec<u8>, SegmentEncodeError> {
    encode_segment_plaintext_iter(
        signature,
        prev_chain,
        entries.len() as u64,
        cipher,
        entries.iter(),
    )
}

pub fn encode_segment_plaintext_iter<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    count: u64,
    cipher: &Option<SegmentCipher>,
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
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
        count,
        signature: *signature,
        prev_chain: *prev_chain,
        encoding: SegmentEncoding::Plaintext(encoding),
    };
    header.encode(&mut w)?;
    let (buf, count2) = if let Some(cipher) = cipher {
        encode_compress_encrypt_entries(cipher, prev_chain, &nonce.unwrap(), entries)
    } else {
        encode_compress_entries(entries)
    }?;
    if count != count2 {
        return Err(SegmentEncodeError::CountMismatch {
            actual: count2,
            expected: count,
        });
    }
    w.write_slice(buf.as_slice());
    Ok(w.finalize())
}

pub fn decode_entries<'a, E>(
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<LogEntry<E>, LogDecodeError>>
where
    E: From<&'a [u8]> + BytesWrapper,
{
    let mut reader = Reader::new(bytes);
    let mut failed = false;
    std::iter::from_fn(move || {
        if failed || reader.is_empty() {
            None
        } else {
            let e = LogEntry::decode(&mut reader);
            if e.is_err() {
                failed = true;
            }
            Some(e)
        }
    })
}

pub fn encode_entries<'a, E>(
    entries: impl Iterator<Item = &'a LogEntry<E>>,
    writer: &mut Writer,
) -> Result<u64, LogEncodeError>
where
    E: BytesWrapper + 'a,
{
    let mut count = 0;
    for e in entries {
        if let LogEntry::IndexedEntry(_) = e {
            count += 1;
        }
        e.encode(writer)?;
    }
    Ok(count)
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
    #[error("entry count mismatch, expected {expected:?}, got {actual:?}")]
    CountMismatch { actual: u64, expected: u64 },
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
    #[error("decompressed segment is too large, max 128mb")]
    CompressionOverflow,
    #[error("entry count mismatch, expected {expected:?}, got {actual:?}")]
    CountMismatch { actual: u64, expected: u64 },
}

fn encode_compress_encrypt_entries<'a>(
    segment_cipher: &SegmentCipher,
    prev_chain: &ChainHash,
    nonce: &[u8],
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, u64), SegmentEncodeError> {
    let (mut inout, count) = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(prev_chain, count, nonce, &mut inout)?;
    Ok((inout, count))
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<(Vec<u8>, u64), SegmentEncodeError> {
    let mut w = Writer::new();
    let count = encode_entries(entries, &mut w)?;
    Ok((zstd::encode_all(w.finalize().as_slice(), 0)?, count))
}

/// 128mb decode limit
const ZSTD_DECODE_LIMIT: u64 = 1u64 << 27;

fn decompress_decode_entries(
    buf: &[u8],
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let mut out = vec![];
    zstd::Decoder::with_buffer(buf)?
        .take(ZSTD_DECODE_LIMIT + 1)
        .read_to_end(&mut out)?;
    if out.len() as u64 > ZSTD_DECODE_LIMIT {
        return Err(SegmentDecodeError::CompressionOverflow);
    }
    let it = decode_entries::<PlaintextBytes>(&out);
    let mut res = vec![];
    for e in it {
        res.push(e?.to_static());
    }
    Ok(res)
}

fn decrypt_decompress_decode_entries(
    segment_cipher: &SegmentCipher,
    prev_chain: &ChainHash,
    count: u64,
    nonce: &[u8],
    buf: &[u8], // TODO we could maybe use parent vec as inout buffer for less alloc in the future
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let mut buf = Vec::from(buf);
    segment_cipher.decrypt_segment(prev_chain, count, nonce, &mut buf)?;
    decompress_decode_entries(buf.as_slice())
}

impl SegmentHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        self.prev_chain.encode(w);
        w.write_var_u64(self.count);
        self.signature.encode(w);
        self.encoding.encode(w);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let prev_chain = ChainHash::decode(r)?;
        let count = r.read_var_u64()?;
        let signature = Signature::decode(r).map_err(SegmentDecodeError::from_sig_decode_err)?;
        let encoding = SegmentEncoding::decode(r)?;
        Ok(Self {
            signature,
            encoding,
            prev_chain,
            count,
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

/// Counts the number of _indexed entries_ in the segment.
pub fn count_entries<'a, B: std::fmt::Debug + 'a>(
    entries: impl Iterator<Item = &'a LogEntry<B>>,
) -> u64 {
    let mut count = 0;
    for e in entries {
        if let LogEntry::IndexedEntry(_) = e {
            count += 1;
        }
    }
    count
}

fn check_entry_count<'a, B: std::fmt::Debug + 'a>(
    entries: impl Iterator<Item = &'a LogEntry<B>>,
    expected: u64,
) -> Result<(), SegmentDecodeError> {
    let actual = count_entries(entries);
    if actual != expected {
        return Err(SegmentDecodeError::CountMismatch { actual, expected });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use secrecy::SecretBox;
    use test_strategy::{Arbitrary, proptest};

    use crate::{
        bytes::PlaintextBytes,
        crypto::{RootKey256, Signature},
        ids::LogId,
        log::{
            ChainHash, LogEntry, OpBatch, PlaintextLogEntry, entries_to_opaque,
            segment::{SegmentEncoding, encode_segment_opaque, encode_segment_plaintext},
        },
    };
    #[cfg(test)]
    use crate::{
        crypto::{SegmentCipher, SegmentCipherSuite},
        log::{
            ChainSeed,
            segment::{SegmentReader, count_entries, encode_segment_plaintext_iter},
        },
    };

    #[derive(Arbitrary, Debug)]
    pub(crate) enum GeneratedEntry {
        Ops(OpBatch<PlaintextBytes<'static>>),
        Signature(Signature),
    }

    #[derive(Arbitrary, Debug)]
    pub(crate) struct GeneratedEntries(Vec<GeneratedEntry>);

    impl GeneratedEntries {
        pub(crate) fn into_entries(self) -> Vec<PlaintextLogEntry<'static>> {
            self.0
                .into_iter()
                .map(move |e| match e {
                    GeneratedEntry::Ops(ops) => {
                        LogEntry::IndexedEntry(crate::log::EntryBody::OpBatch(ops))
                    }
                    GeneratedEntry::Signature(signature) => LogEntry::Signature(signature),
                })
                .collect::<Vec<_>>()
        }
    }

    #[proptest(cases = 10)]
    fn test_segments_no_cipher(
        #[strategy(0u64..1<<24)] start_idx: u64,
        entries: GeneratedEntries,
        log_id: LogId,
    ) {
        let entries = entries.into_entries();
        let count = count_entries(entries.iter());
        let seed = ChainSeed::new(&log_id);
        let start_chain = ChainHash {
            size: start_idx,
            hash: [3; 32],
        };
        let sig = Signature::Ed25519([2; 64]);

        // test opaque encoding
        {
            let (opaque, _) =
                entries_to_opaque(&None, &seed, &start_chain, entries.iter()).unwrap();
            let segment = encode_segment_opaque(&sig, &start_chain, count, opaque.iter()).unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(count, header.count);
            assert_eq!(start_chain, header.prev_chain);
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
                encode_segment_plaintext_iter(&sig, &start_chain, count, &None, entries.iter())
                    .unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(count, header.count);
            assert_eq!(start_chain, header.prev_chain);
            let decoded = reader.read(&None).unwrap();
            match decoded {
                crate::log::segment::DecodedSegment::Plaintext(items) => {
                    assert_eq!(entries, items);
                }
                crate::log::segment::DecodedSegment::Opaque(_) => unreachable!(),
            }
        }
    }

    #[proptest(cases = 10)]
    fn test_segments_with_cipher(
        #[strategy(0u64..1<<24)] start_idx: u64,
        // safe to use random entries which include UseKey entries since we're not doing per-entry encryption
        entries: Vec<PlaintextLogEntry<'static>>,
        log_id: LogId,
        key: [u8; 32],
    ) {
        let key = RootKey256::new(SecretBox::new(Box::new(key)));
        let cipher = Some(SegmentCipher::new(
            SegmentCipherSuite::XChaCha20Poly1305,
            key.container_key(&log_id.container_id),
            &log_id,
        ));
        let count = count_entries(entries.iter());
        let chain_start = ChainHash {
            size: start_idx,
            hash: [3; 32],
        };
        let sig = Signature::Ed25519([2; 64]);

        let segment =
            encode_segment_plaintext_iter(&sig, &chain_start, count, &cipher, entries.iter())
                .unwrap();
        let reader = SegmentReader::start(&segment).unwrap();
        let header = reader.header();
        assert_eq!(sig, header.signature);
        assert_eq!(count, header.count);
        assert_eq!(chain_start, header.prev_chain);
        let decoded = reader.read(&cipher).unwrap();
        match decoded {
            crate::log::segment::DecodedSegment::Plaintext(items) => {
                assert_eq!(entries, items);
            }
            crate::log::segment::DecodedSegment::Opaque(_) => unreachable!(),
        }
    }
}
