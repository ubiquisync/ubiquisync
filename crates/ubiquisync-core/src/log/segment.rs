mod join;

use std::io::Read;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{
        CipherError, CipherInfo, CipherKeyResolver, CryptoDecodeError, SegmentCipher,
        SegmentCipherSuite, Signature, SignatureVerifyError, VerifyingKey,
    },
    ids::LogId,
    log::{
        ChainHash, ChainSeed, LogDecodeError, LogEncodeError, LogEntry, LogValidationError,
        LogVerifyError, OpaqueLogEntry, PlaintextLogEntry, SegmentCipherError,
        entries_to_plaintext, verify_opaque, verify_plaintext,
    },
};

use super::EntryBody;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub signature: Signature,
    pub prev_chain: ChainHash,
    pub active_cipher: Option<CipherInfo>,
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
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub enum Compression {
    Zstd = 0,
}

pub struct VerifiedSegment<'a> {
    pub decoded: DecodedSegment<'a>,
    pub head_chain: ChainHash,
    pub head_cipher: Option<CipherInfo>,
    pub chain_seed: ChainSeed,
}

pub struct DecodedSegment<'a> {
    pub header: SegmentHeader,
    pub entries: DecodedEntries<'a>,
}

pub enum DecodedEntries<'a> {
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

    pub async fn read(
        self,
        key_resolver: &dyn CipherKeyResolver,
        log_id: &LogId,
    ) -> Result<DecodedSegment<'a>, SegmentDecodeError> {
        let buf = self.reader.into_remaining();
        let header = self.header;
        let entries = match &header.encoding {
            SegmentEncoding::Opaque => {
                let mut res = vec![];
                for e in decode_entries(buf) {
                    res.push(e?);
                }
                DecodedEntries::Opaque(res)
            }
            SegmentEncoding::Plaintext(enc) => {
                DecodedEntries::Plaintext(match &enc.outer_encryption {
                    Some(c) => {
                        let key = key_resolver
                            .resolve_container_key(&c.cipher.fingerprint, &log_id.container_id)
                            .await
                            .ok_or(SegmentDecodeError::MissingSegmentCipher(c.cipher))?;
                        let suite =
                            SegmentCipherSuite::try_from(c.cipher.cipher_suite).map_err(|_| {
                                SegmentDecodeError::UnknownCipherSuite(c.cipher.cipher_suite)
                            })?;
                        let cipher = SegmentCipher::new(suite, key, log_id);
                        decrypt_decompress_decode_entries(
                            &cipher,
                            &header.prev_chain,
                            &c.nonce,
                            buf,
                        )?
                    }
                    None => decompress_decode_entries(buf)?,
                })
            }
        };
        Ok(DecodedSegment { header, entries })
    }

    pub async fn verify(
        self,
        verifying_key: &VerifyingKey,
        key_resolver: &dyn CipherKeyResolver,
        seed: &ChainSeed,
    ) -> Result<VerifiedSegment<'a>, SegmentVerifyError> {
        let header = self.header.clone();
        let decoded = self.read(key_resolver, seed.log_id()).await?;
        let mut cipher = header.active_cipher;
        let chain_hash = match decoded.entries {
            DecodedEntries::Opaque(ref entries) => {
                // make sure we update the end cipher here too
                let chain_hash =
                    verify_opaque(verifying_key, seed, &header.prev_chain, entries.iter())?;
                for e in entries.iter() {
                    if let LogEntry::IndexedEntry(EntryBody::UseKey(ci)) = e {
                        cipher = Some(*ci)
                    }
                }
                chain_hash
            }
            DecodedEntries::Plaintext(ref entries) => {
                verify_plaintext(
                    verifying_key,
                    seed,
                    &mut cipher,
                    &header.prev_chain,
                    key_resolver,
                    entries.iter(),
                )
                .await?
            }
        };
        verifying_key.verify_signature(&chain_hash.sign_bytes(seed), &header.signature)?;
        Ok(VerifiedSegment {
            decoded,
            head_chain: chain_hash,
            head_cipher: cipher,
            chain_seed: *seed,
        })
    }
}

#[derive(Error, Debug)]
pub enum SegmentVerifyError {
    #[error("decode error: {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("signature: {0}")]
    Signature(#[from] SignatureVerifyError),
    #[error("signature: {0}")]
    Verify(#[from] LogVerifyError),
}

pub fn encode_segment_opaque<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    entries: impl Iterator<Item = &'a OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let header = SegmentHeader {
        prev_chain: *prev_chain,
        active_cipher: *start_cipher,
        signature: *signature,
        encoding: SegmentEncoding::Opaque,
    };
    header.encode(&mut w)?;
    encode_entries(entries, &mut w)?;
    Ok(w.finalize())
}

pub fn encode_segment_plaintext<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
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
        signature: *signature,
        prev_chain: *prev_chain,
        active_cipher: *start_cipher,
        encoding: SegmentEncoding::Plaintext(encoding),
    };
    header.encode(&mut w)?;
    let buf = if let Some(cipher) = cipher {
        encode_compress_encrypt_entries(cipher, prev_chain, &nonce.unwrap(), entries)
    } else {
        encode_compress_entries(entries)
    }?;
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
) -> Result<(), LogEncodeError>
where
    E: BytesWrapper + 'a,
{
    for e in entries {
        e.encode(writer)?;
    }
    Ok(())
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
    #[error("unknown segment encoding {0}")]
    UnknownSegmentEncoding(u8),
    #[error("unknown compression {0}")]
    UnknownCompression(u8),
    #[error("unknown encryption info type: {0}")]
    UnknownEncryptionInfo(u8),
    #[error("missing segment cipher {0:?}")]
    MissingSegmentCipher(CipherInfo),
    #[error("unknown cipher suite {0}")]
    UnknownCipherSuite(u8),
    #[error("decompressed segment is too large, max 128mb")]
    CompressionOverflow,
    #[error("entry validation error: {0}")]
    Validation(#[from] LogValidationError),
}

fn encode_compress_encrypt_entries<'a>(
    segment_cipher: &SegmentCipher,
    prev_chain: &ChainHash,
    nonce: &[u8],
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut inout = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(prev_chain, nonce, &mut inout)?;
    Ok(inout)
}

fn encode_compress_entries<'a>(
    entries: impl Iterator<Item = &'a PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    encode_entries(entries, &mut w)?;
    Ok(zstd::encode_all(w.finalize().as_slice(), 0)?)
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
        let e = e?;
        e.validate()?;
        res.push(e.to_static());
    }
    Ok(res)
}

fn decrypt_decompress_decode_entries(
    segment_cipher: &SegmentCipher,
    prev_chain: &ChainHash,
    nonce: &[u8],
    buf: &[u8], // TODO we could maybe use parent vec as inout buffer for less alloc in the future
) -> Result<Vec<PlaintextLogEntry<'static>>, SegmentDecodeError> {
    let mut buf = Vec::from(buf);
    segment_cipher.decrypt_segment(prev_chain, nonce, &mut buf)?;
    decompress_decode_entries(buf.as_slice())
}

impl SegmentHeader {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        self.prev_chain.encode(w);
        self.signature.encode(w);
        self.encoding.encode(w)?;
        if let Some(ci) = self.active_cipher {
            w.write_byte(1);
            ci.encode(w);
        } else {
            w.write_byte(0);
        }
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let prev_chain = ChainHash::decode(r)?;
        let signature = Signature::decode(r).map_err(SegmentDecodeError::from_sig_decode_err)?;
        let encoding = SegmentEncoding::decode(r)?;
        let active_cipher = if r.read_byte()? == 1 {
            // have cipher
            Some(CipherInfo::decode(r)?)
        } else {
            // TODO: we could reject bytes that aren't 0 or 1
            None
        };
        Ok(Self {
            signature,
            encoding,
            prev_chain,
            active_cipher,
        })
    }
}

impl SegmentEncoding {
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        match self {
            SegmentEncoding::Opaque => {
                w.write_byte(SEGMENT_ENCODING_OPAQUE);
            }
            SegmentEncoding::Plaintext(enc) => {
                w.write_byte(SEGMENT_ENCODING_PLAINTEXT);
                enc.encode(w);
            }
        }
        Ok(())
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

impl<'a> VerifiedSegment<'a> {
    pub async fn to_plaintext(
        self,
        head_cipher: &mut Option<CipherInfo>,
        key_resolver: &dyn CipherKeyResolver,
    ) -> Result<Vec<PlaintextLogEntry<'a>>, SegmentCipherError> {
        let VerifiedSegment {
            chain_seed,
            decoded,
            ..
        } = self;
        let header = decoded.header;
        let res = decoded
            .entries
            .to_plaintext(&chain_seed, head_cipher, &header.prev_chain, key_resolver)
            .await?;
        Ok(res)
    }
}

impl<'a> DecodedEntries<'a> {
    pub async fn to_plaintext(
        self,
        seed: &ChainSeed,
        head_cipher: &mut Option<CipherInfo>,
        head_chain: &ChainHash,
        key_resolver: &dyn CipherKeyResolver,
    ) -> Result<Vec<PlaintextLogEntry<'a>>, SegmentCipherError> {
        match self {
            DecodedEntries::Opaque(items) => {
                let e =
                    entries_to_plaintext(seed, head_cipher, head_chain, key_resolver, items.iter())
                        .await?;
                Ok(e.into_iter().map(|e| e.0).collect())
            }
            DecodedEntries::Plaintext(items) => Ok(items),
        }
    }
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
            segment::{SegmentEncoding, encode_segment_opaque},
        },
    };
    #[cfg(test)]
    use crate::{
        crypto::{SegmentCipher, SegmentCipherSuite},
        log::{
            ChainSeed,
            segment::{SegmentReader, encode_segment_plaintext_iter},
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
            let segment = encode_segment_opaque(&sig, &start_chain, opaque.iter()).unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(start_chain, header.prev_chain);
            assert_eq!(header.encoding, SegmentEncoding::Opaque);
            let decoded = reader.read(&None).unwrap();
            match decoded {
                crate::log::segment::DecodedEntries::Opaque(items) => {
                    assert_eq!(opaque, items);
                }
                crate::log::segment::DecodedEntries::Plaintext(_) => unreachable!(),
            }
        }

        // test plaintext encoding (basically just compression)
        {
            let segment =
                encode_segment_plaintext_iter(&sig, &start_chain, &None, entries.iter()).unwrap();
            let reader = SegmentReader::start(&segment).unwrap();
            let header = reader.header();
            assert_eq!(sig, header.signature);
            assert_eq!(start_chain, header.prev_chain);
            let decoded = reader.read(&None).unwrap();
            match decoded {
                crate::log::segment::DecodedEntries::Plaintext(items) => {
                    assert_eq!(entries, items);
                }
                crate::log::segment::DecodedEntries::Opaque(_) => unreachable!(),
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
        let chain_start = ChainHash {
            size: start_idx,
            hash: [3; 32],
        };
        let sig = Signature::Ed25519([2; 64]);

        let segment =
            encode_segment_plaintext_iter(&sig, &chain_start, &cipher, entries.iter()).unwrap();
        let reader = SegmentReader::start(&segment).unwrap();
        let header = reader.header();
        assert_eq!(sig, header.signature);
        assert_eq!(chain_start, header.prev_chain);
        let decoded = reader.read(&cipher).unwrap();
        match decoded {
            crate::log::segment::DecodedEntries::Plaintext(items) => {
                assert_eq!(entries, items);
            }
            crate::log::segment::DecodedEntries::Opaque(_) => unreachable!(),
        }
    }
}
