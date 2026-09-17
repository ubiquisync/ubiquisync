mod join;

use std::io::Read;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{
        CipherError, CipherInfo, CipherKeyResolver, CipherSuite, CryptoDecodeError, SegmentCipher,
        Signature, SignatureVerifyError, VerifyingKey,
    },
    ids::LogId,
    log::{
        ChainHash, ChainSeed, LogDecodeError, LogEncodeError, LogEntry, LogValidationError,
        LogVerifyError, OpaqueLogEntry, PlaintextLogEntry, SegmentCipherError, verify_opaque,
        verify_plaintext,
    },
};

use super::EntryBody;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub signature: Signature,
    pub prev_chain: ChainHash,
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
    /// Indicates whether to use a different cipher or even no cipher,
    /// at the start of the segment for hashing.

    /// Specifies which cipher to start hashing with.
    pub start_cipher: StartCipher,

    /// The cipher used to encrypt the whole segment.
    /// This will _always_ be the last cipher used in the segment
    /// if there were any cipher changes.
    pub segment_cipher: CipherInfo,
    pub nonce: Vec<u8>,
}

/// Specifies which cipher to start hashing plaintext segments with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartCipher {
    /// Specifies that we can start hashing with no cipher, either:
    /// - the segment starts unencrypted (and then gets a cipher later), or
    /// - the first entry is `UseKey` so started with a cipher is unneeded
    None,
    /// Start hashing with the segment cipher.
    SegmentCipher,
    /// Start hashing with this other cipher.
    /// This should be used if the segment starts with some other cipher, but
    /// switches to the cipher specified by [EncryptionInfo::segment_cipher]
    /// by the end.
    Other(CipherInfo),
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
    pub chain_seed: ChainSeed,
}

pub struct DecodedSegment<'a> {
    pub header: SegmentHeader,
    pub entries: DecodedEntries<'a>,
    pub log_id: LogId,
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
                    Some(enc) => {
                        let segment_cipher = enc.segment_cipher;
                        let suite =
                            CipherSuite::try_from(segment_cipher.cipher_suite).map_err(|_| {
                                SegmentDecodeError::UnknownCipherSuite(segment_cipher.cipher_suite)
                            })?;
                        let key = key_resolver
                            .resolve_container_key(
                                &segment_cipher.fingerprint,
                                &log_id.container_id,
                            )
                            .await
                            .ok_or(SegmentDecodeError::MissingSegmentKey(segment_cipher))?;
                        let cipher = SegmentCipher::new(suite, key, log_id);
                        decrypt_decompress_decode_entries(
                            &cipher,
                            &header.prev_chain,
                            &enc.nonce,
                            buf,
                        )?
                    }
                    None => decompress_decode_entries(buf)?,
                })
            }
        };
        Ok(DecodedSegment {
            header,
            entries,
            log_id: *log_id,
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

pub fn encode_segment_opaque<'a: 'b, 'b>(
    signature: &Signature,
    prev_chain: &ChainHash,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let header = SegmentHeader {
        signature: *signature,
        prev_chain: *prev_chain,
        encoding: SegmentEncoding::Opaque,
    };
    header.encode(&mut w)?;
    encode_entries(entries, &mut w)?;
    Ok(w.finalize())
}

pub async fn encode_segment_plaintext<'a>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    log_id: &LogId,
    key_resolver: &dyn CipherKeyResolver,
    entries: &[PlaintextLogEntry<'a>],
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    let (header, cipher) = SegmentHeader::init_plaintext(
        *signature,
        *prev_chain,
        *start_cipher,
        log_id,
        key_resolver,
        entries,
    )
    .await?;
    header.encode(&mut w)?;
    let buf = if let Some((cipher, nonce)) = cipher {
        encode_compress_encrypt_entries(&cipher, prev_chain, &nonce, entries.iter())
    } else {
        encode_compress_entries(entries.iter())
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
    #[error("missing segment key {0:?}")]
    MissingSegmentKey(CipherInfo),
    #[error("unknown cipher suite: {0}")]
    UnknownCipherSuite(u8),
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
    #[error("missing segment key {0:?}")]
    MissingSegmentKey(CipherInfo),
    #[error("missing segment cipher")]
    MissingSegmentCipher,
    #[error("unknown cipher suite {0}")]
    UnknownCipherSuite(u8),
    #[error("decompressed segment is too large, max 128mb")]
    CompressionOverflow,
    #[error("entry validation error: {0}")]
    Validation(#[from] LogValidationError),
    #[error("unknown start cipher tag {0}")]
    UnknownStartCipherTag(u8),
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
    async fn init_plaintext(
        signature: Signature,
        prev_chain: ChainHash,
        start_cipher: Option<CipherInfo>,
        log_id: &LogId,
        key_resolver: &dyn CipherKeyResolver,
        entries: &[PlaintextLogEntry<'_>],
    ) -> Result<(Self, Option<(SegmentCipher, Vec<u8>)>), SegmentEncodeError> {
        let mut end_cipher = start_cipher;
        for e in entries {
            if let LogEntry::IndexedEntry(EntryBody::UseKey(ci)) = e {
                end_cipher = Some(*ci);
            }
        }

        let start_cipher = match start_cipher {
            None => StartCipher::None,
            Some(ci) => {
                if let Some(LogEntry::IndexedEntry(EntryBody::UseKey(_))) = entries.first() {
                    StartCipher::None
                } else if Some(ci) == end_cipher {
                    StartCipher::SegmentCipher
                } else {
                    StartCipher::Other(ci)
                }
            }
        };

        let (enc, cipher) = if let Some(ci) = end_cipher {
            let cipher_suite = CipherSuite::try_from(ci.cipher_suite)
                .map_err(|_| SegmentEncodeError::UnknownCipherSuite(ci.cipher_suite))?;

            let nonce_size = cipher_suite.segment_nonce_size();
            let mut nonce = vec![0; nonce_size];
            getrandom::fill(nonce.as_mut_slice())
                .map_err(|_| SegmentEncodeError::NonceGenerationError)?;

            let key = key_resolver
                .resolve_container_key(&ci.fingerprint, &log_id.container_id)
                .await
                .ok_or(SegmentEncodeError::MissingSegmentKey(ci))?;
            let segment_cipher = SegmentCipher::new(cipher_suite, key, log_id);

            (
                Some(EncryptionInfo {
                    start_cipher,
                    segment_cipher: ci,
                    nonce: nonce.clone(),
                }),
                Some((segment_cipher, nonce)),
            )
        } else {
            (None, None)
        };

        Ok((
            Self {
                signature,
                prev_chain,
                encoding: SegmentEncoding::Plaintext(PlaintextSegmentEncoding {
                    outer_encryption: enc,
                    inner_compression: Compression::Zstd,
                }),
            },
            cipher,
        ))
    }

    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        self.prev_chain.encode(w);
        self.signature.encode(w);
        self.encoding.encode(w)?;
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let prev_chain = ChainHash::decode(r)?;
        let signature = Signature::decode(r).map_err(SegmentDecodeError::from_sig_decode_err)?;
        let encoding = SegmentEncoding::decode(r)?;
        Ok(Self {
            signature,
            encoding,
            prev_chain,
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
        w.write_option(&self.outer_encryption, |w, e| e.encode(w));
        w.write_byte(self.inner_compression.into());
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let outer_encryption = r.read_option(|r| EncryptionInfo::decode(r))?;
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
        self.start_cipher.encode(w);
        self.segment_cipher.encode(w);
        w.write_len_prefixed(&self.nonce);
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let start_cipher = StartCipher::decode(r)?;
        let segment_cipher = CipherInfo::decode(r)?;
        let nonce = r.read_len_prefixed()?;
        Ok(Self {
            start_cipher,
            segment_cipher,
            nonce: nonce.into(),
        })
    }
}

impl StartCipher {
    pub fn encode(&self, w: &mut Writer) {
        match self {
            StartCipher::None => w.write_byte(0),
            StartCipher::SegmentCipher => w.write_byte(1),
            StartCipher::Other(cipher_info) => {
                w.write_byte(2);
                cipher_info.encode(w);
            }
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let b = r.read_byte()?;
        Ok(match b {
            0 => Self::None,
            1 => Self::SegmentCipher,
            2 => Self::Other(CipherInfo::decode(r)?),
            _ => return Err(SegmentDecodeError::UnknownStartCipherTag(b)),
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

impl<'a> DecodedSegment<'a> {
    pub async fn verify(
        self,
        verifying_key: &VerifyingKey,
        key_resolver: &dyn CipherKeyResolver,
    ) -> Result<VerifiedSegment<'a>, SegmentVerifyError> {
        let header = &self.header;
        let seed = ChainSeed::new(&self.log_id);
        let chain_hash = match self.entries {
            DecodedEntries::Opaque(ref entries) => {
                verify_opaque(verifying_key, &seed, &header.prev_chain, entries.iter())?
            }
            DecodedEntries::Plaintext(ref entries) => {
                let mut cipher = if let SegmentEncoding::Plaintext(PlaintextSegmentEncoding {
                    outer_encryption: Some(ref enc),
                    ..
                }) = header.encoding
                {
                    match enc.start_cipher {
                        StartCipher::None => None,
                        StartCipher::SegmentCipher => Some(enc.segment_cipher),
                        StartCipher::Other(cipher_info) => Some(cipher_info),
                    }
                } else {
                    None
                };
                verify_plaintext(
                    verifying_key,
                    &seed,
                    &mut cipher,
                    &header.prev_chain,
                    key_resolver,
                    entries.iter(),
                )
                .await?
            }
        };
        verifying_key.verify_signature(&chain_hash.sign_bytes(&seed), &header.signature)?;
        Ok(VerifiedSegment {
            decoded: self,
            head_chain: chain_hash,
            chain_seed: seed,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;

    use secrecy::SecretBox;
    use test_strategy::{Arbitrary, proptest};

    use crate::{
        bytes::PlaintextBytes,
        crypto::{
            CipherInfo, CipherKeyResolver, CipherSuite, ContainerKey256, RootKey256,
            RootKey256Fingerprint, Signature, SigningKey, VerifyingKey, ed25519::Ed25519SigningKey,
        },
        ids::{ContainerId, LogId},
        log::{
            ChainHash, EntryBody, LogEntry, OpBatch, PlaintextLogEntry, entries_to_opaque,
            entries_to_plaintext,
            segment::{encode_segment_opaque, encode_segment_plaintext},
            track_cipher_change,
        },
    };

    use crate::log::{ChainSeed, segment::SegmentReader};

    #[derive(Debug, Arbitrary)]
    enum TestEntry {
        #[weight(5)]
        Ops(OpBatch<PlaintextBytes<'static>>),
        #[weight(2)]
        UseKey([u8; 32]),
        #[weight(1)]
        Signature,
    }

    #[derive(Debug, Arbitrary)]
    struct TestCase {
        start_key: Option<[u8; 32]>,
        entries: Vec<TestEntry>,
        signing_key: [u8; 32],
        log_id: LogId,
        prev_chain: ChainHash,
    }

    struct TestCaseData {
        start_cipher: Option<CipherInfo>,
        end_cipher: Option<CipherInfo>,
        log_id: LogId,
        prev_chain: ChainHash,
        head_chain: ChainHash,
        entries: Vec<PlaintextLogEntry<'static>>,
        key_resolver: TestKeyResolver,
        signature: Signature,
        verifying_key: VerifyingKey,
        seed: ChainSeed,
    }

    type TestKeyResolver = HashMap<RootKey256Fingerprint, RootKey256>;

    impl TestCase {
        async fn data(&self) -> TestCaseData {
            let mut entries = vec![];
            let signing_key = Ed25519SigningKey::new(SecretBox::new(Box::new(self.signing_key)));
            let mut key_resolver = TestKeyResolver::new();
            let prev_chain = self.prev_chain;
            let mut head_chain = prev_chain;
            let switch_key = |k: [u8; 32], kr: &mut TestKeyResolver| {
                let k = RootKey256::new(SecretBox::new(Box::new(k)));
                let fingerprint = *k.fingerprint();
                kr.insert(fingerprint, k);
                CipherInfo {
                    cipher_suite: CipherSuite::ChaCha20.into(),
                    fingerprint,
                }
            };
            let start_cipher = self.start_key.map(|k| switch_key(k, &mut key_resolver));
            let mut head_cipher = start_cipher;
            let seed = ChainSeed::new(&self.log_id);
            for e in self.entries.iter() {
                let e = match e {
                    TestEntry::Ops(op_batch) => {
                        LogEntry::IndexedEntry(EntryBody::OpBatch(op_batch.clone()))
                    }
                    TestEntry::UseKey(k) => {
                        LogEntry::IndexedEntry(EntryBody::UseKey(switch_key(*k, &mut key_resolver)))
                    }
                    TestEntry::Signature => LogEntry::Signature(
                        signing_key.sign(&head_chain.sign_bytes(&seed)).unwrap(),
                    ),
                };
                head_chain = head_chain
                    .compute_next_plaintext(
                        &seed,
                        &mut head_cipher,
                        &key_resolver,
                        [e.clone()].iter(),
                    )
                    .await
                    .unwrap();
                entries.push(e);
            }
            TestCaseData {
                start_cipher,
                end_cipher: head_cipher,
                log_id: self.log_id,
                head_chain,
                entries,
                key_resolver,
                signature: signing_key.sign(&head_chain.sign_bytes(&seed)).unwrap(),
                verifying_key: signing_key.verifying_key(),
                prev_chain,
                seed,
            }
        }
    }

    #[async_trait::async_trait]
    impl CipherKeyResolver for HashMap<RootKey256Fingerprint, RootKey256> {
        async fn resolve_container_key(
            &self,
            fingerprint: &RootKey256Fingerprint,
            container_id: &ContainerId,
        ) -> Option<ContainerKey256> {
            self.get(fingerprint).map(|k| k.container_key(container_id))
        }
    }

    #[proptest(async = "tokio", cases = 10)]
    async fn roundtrip_segments(case: TestCase) {
        let data = case.data().await;
        let plaintext_segment = encode_segment_plaintext(
            &data.signature,
            &data.prev_chain,
            &data.start_cipher,
            &data.log_id,
            &data.key_resolver,
            &data.entries,
        )
        .await
        .unwrap();
        check_decode(&plaintext_segment, &data).await;

        let mut head_cipher = data.start_cipher;
        let opaque = entries_to_opaque(
            &data.seed,
            &mut head_cipher,
            &data.prev_chain,
            &data.key_resolver,
            data.entries.iter(),
        )
        .await
        .unwrap();
        assert_eq!(data.end_cipher, head_cipher);
        // this assertion is weirdly tolerant of empty segments which we maybe should reject, but currently they're sort of benign
        assert_eq!(
            data.head_chain,
            opaque.last().map(|e| e.1).unwrap_or(data.prev_chain)
        );

        let opaque_segment = encode_segment_opaque(
            &data.signature,
            &data.prev_chain,
            opaque.iter().map(|(e, _)| e),
        )
        .unwrap();
        check_decode(&opaque_segment, &data).await;
    }

    async fn check_decode(segment: &[u8], data: &TestCaseData) {
        let reader = SegmentReader::start(segment).unwrap();
        let decoded = reader.read(&data.key_resolver, &data.log_id).await.unwrap();
        let verified = decoded
            .verify(&data.verifying_key, &data.key_resolver)
            .await
            .unwrap();
        assert_eq!(data.prev_chain, verified.decoded.header.prev_chain);
        assert_eq!(data.head_chain, verified.head_chain);
        match verified.decoded.entries {
            crate::log::segment::DecodedEntries::Opaque(items) => {
                let mut head_cipher = data.start_cipher;
                let plaintext = entries_to_plaintext(
                    &data.seed,
                    &mut head_cipher,
                    &data.prev_chain,
                    &data.key_resolver,
                    items.iter(),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|e| e.0)
                .collect::<Vec<_>>();
                assert_eq!(data.entries, plaintext);
                assert_eq!(data.end_cipher, head_cipher);
            }
            crate::log::segment::DecodedEntries::Plaintext(items) => {
                assert_eq!(data.entries, items);

                let mut head_cipher = data.start_cipher;
                track_cipher_change(&mut head_cipher, items.iter());
                assert_eq!(data.end_cipher, head_cipher);

                let mut head_cipher = data.start_cipher;
                let head_chain = data
                    .prev_chain
                    .compute_next_plaintext(
                        &data.seed,
                        &mut head_cipher,
                        &data.key_resolver,
                        items.iter(),
                    )
                    .await
                    .unwrap();
                assert_eq!(data.end_cipher, head_cipher);
                assert_eq!(data.head_chain, head_chain);
            }
        }
    }
}
