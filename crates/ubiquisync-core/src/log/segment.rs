mod join;

pub use join::*;

use std::io::Read;

use itertools::Itertools;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use thiserror::Error;

use crate::{
    bytes::{BytesWrapper, PlaintextBytes, ToStatic},
    codec::{ReadError, Reader, WriteError, Writer},
    crypto::{
        CipherError, CipherInfo, CipherKeyResolver, CipherSuite, CryptoDecodeError, EntryCipher,
        SegmentCipher, Signature, SignatureVerifyError, VerifyingKey,
    },
    hlc::Timestamp,
    ids::LogId,
    log::{
        ChainHash, ChainHashError, DecodeTimestamp, LogDecodeError, LogEncodeError, LogEntry,
        LogHashContext, LogValidationError, OpaqueLogEntry, PlaintextLogEntry, SegmentCipherError,
        TimestampRepr, check_cipher_change, entries_to_opaque, to_plaintext,
    },
};

use super::EntryBody;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentHeader {
    pub signature: Signature,
    pub prev_chain: ChainHash,
    /// The cipher used at the start of the segment (if any).
    /// This field is omitted if the first entry is a `UseKey`
    /// entry which sets the cipher.
    pub start_cipher: Option<CipherInfo>,
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
    /// The cipher for the whole compressed, plaintext segment payload.
    /// If there is a cipher change in the middle of a segment, the
    /// last entry cipher should be used.
    pub cipher: Option<CipherInfo>,
    pub nonce: Vec<u8>,
}

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "proptest", derive(test_strategy::Arbitrary))]
pub enum Compression {
    Zstd = 0,
}

pub struct DecodedSegment<'a> {
    pub header: SegmentHeader,
    pub chain_seed: LogHashContext,
    pub entries: Vec<DecodedEntry<'a>>,
    pub head_chain: ChainHash,
    pub head_cipher: Option<CipherInfo>,
}

pub struct DecodedEntry<'a> {
    pub chain_meta: ChainMeta,
    pub body: DecodedEntryBody<'a>,
}

pub enum DecodedEntryBody<'a> {
    Opaque(OpaqueLogEntry<'a>),
    Plaintext(PlaintextLogEntry<'a>),
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
        hash_ctx: &LogHashContext,
    ) -> Result<DecodedSegment<'a>, SegmentDecodeError> {
        let buf = self.reader.into_remaining();
        let header = self.header;
        let log_id = hash_ctx.log_id();
        let mut entries = vec![];
        let mut active_cipher = header.start_cipher;
        let mut chain_hash = header.prev_chain;
        match &header.encoding {
            SegmentEncoding::Opaque => {
                for e in decode_entries(buf) {
                    let e = e?;
                    chain_hash = chain_hash.next(&e, hash_ctx, &mut active_cipher)?;
                    entries.push(DecodedEntry {
                        chain_meta: ChainMeta {
                            chain_hash,
                            cipher_info: active_cipher,
                        },
                        body: DecodedEntryBody::Opaque(e),
                    });
                }
            }
            SegmentEncoding::Plaintext(enc) => {
                let plaintext = match &enc.outer_encryption {
                    Some(c) => {
                        let ci = c
                            .cipher
                            .or(header.start_cipher)
                            .ok_or(SegmentDecodeError::MissingSegmentCipher)?;
                        let suite = CipherSuite::try_from(ci.cipher_suite)
                            .map_err(|_| SegmentDecodeError::UnknownCipherSuite(ci.cipher_suite))?;
                        let key = key_resolver
                            .resolve_container_key(&ci.fingerprint, &log_id.container_id)
                            .await
                            .ok_or(SegmentDecodeError::MissingSegmentKey(ci))?;
                        let cipher = SegmentCipher::new(suite, key, &log_id.peer_id);
                        decrypt_decompress_decode_entries(
                            &cipher,
                            &header.prev_chain,
                            &c.nonce,
                            buf,
                        )?
                    }
                    None => decompress_decode_entries(buf)?,
                };
                let opaque = entries_to_opaque(
                    hash_ctx,
                    &mut active_cipher,
                    &chain_hash,
                    key_resolver,
                    plaintext.iter(),
                )
                .await;
                for (op_res, pl) in opaque.into_iter().zip(plaintext) {
                    let (_, chain_meta) = op_res?;
                    chain_hash = chain_meta.chain_hash;
                    entries.push(DecodedEntry {
                        chain_meta,
                        body: DecodedEntryBody::Plaintext(pl),
                    })
                }
            }
        };
        Ok(DecodedSegment {
            header,
            entries,
            chain_seed: hash_ctx.clone(),
            head_chain: chain_hash,
            head_cipher: active_cipher,
        })
    }
}

#[derive(Error, Debug)]
pub enum SegmentVerifyError {
    // TODO we want to pull these errors to the top level:
    // - missing key
    // - unknown algorithms/tags
    #[error("decode error: {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("cipher error: {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("signature: {0}")]
    Signature(#[from] SignatureVerifyError),
}

pub fn encode_segment_opaque<'a: 'b, 'b>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    encode_segment_opaque_writer(signature, prev_chain, start_cipher, entries, &mut w)?;
    Ok(w.finalize())
}

pub fn encode_segment_opaque_writer<'a: 'b, 'b>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    entries: impl Iterator<Item = &'b OpaqueLogEntry<'a>>,
    w: &mut Writer,
) -> Result<(), SegmentEncodeError> {
    let mut entries = entries.peekable();
    let header = SegmentHeader::init_opaque(*signature, *prev_chain, *start_cipher, entries.peek());
    header.encode(w)?;
    encode_entries(entries, w)?;
    Ok(())
}

pub async fn encode_segment_plaintext<'a: 'b, 'b>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    log_id: &LogId,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>> + Clone,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut w = Writer::new();
    encode_segment_plaintext_writer(
        signature,
        prev_chain,
        start_cipher,
        log_id,
        key_resolver,
        entries,
        &mut w,
    )
    .await?;
    Ok(w.finalize())
}

pub async fn encode_segment_plaintext_writer<'a: 'b, 'b>(
    signature: &Signature,
    prev_chain: &ChainHash,
    start_cipher: &Option<CipherInfo>,
    log_id: &LogId,
    key_resolver: &dyn CipherKeyResolver,
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>> + Clone,
    w: &mut Writer,
) -> Result<(), SegmentEncodeError> {
    let (header, cipher) = SegmentHeader::init_plaintext(
        *signature,
        *prev_chain,
        *start_cipher,
        log_id,
        key_resolver,
        entries.clone(),
    )
    .await?;
    header.encode(w)?;
    let buf = if let Some((cipher, nonce)) = cipher {
        encode_compress_encrypt_entries(&cipher, prev_chain, &nonce, entries)
    } else {
        encode_compress_entries(entries)
    }?;
    w.write_slice(buf.as_slice());
    Ok(())
}

pub fn decode_entries<'a, B, T>(
    bytes: &'a [u8],
) -> impl Iterator<Item = Result<LogEntry<B, T>, LogDecodeError>>
where
    B: From<&'a [u8]> + BytesWrapper,
    T: DecodeTimestamp<'a>,
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

pub fn encode_entries<'a, B, T>(
    entries: impl Iterator<Item = &'a LogEntry<B, T>>,
    writer: &mut Writer,
) -> Result<(), LogEncodeError>
where
    B: BytesWrapper + 'a,
    T: TimestampRepr + 'a,
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
    #[error("start size {0} out of range")]
    StartSizeOutOfRange(u64),
}

#[derive(Error, Debug)]
pub enum SegmentDecodeError {
    #[error("entry decode error: {0}")]
    LogDecodeError(#[from] LogDecodeError),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("cipher error: {0}")]
    Cipher(#[from] CipherError),
    #[error("segment cipher error: {0}")]
    SegmentCipher(#[from] SegmentCipherError),
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
    #[error("hash error: {0}")]
    Hash(#[from] ChainHashError),
}

fn encode_compress_encrypt_entries<'a: 'b, 'b>(
    segment_cipher: &SegmentCipher,
    prev_chain: &ChainHash,
    nonce: &[u8],
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
) -> Result<Vec<u8>, SegmentEncodeError> {
    let mut inout = encode_compress_entries(entries)?;
    segment_cipher.encrypt_segment(prev_chain, nonce, &mut inout)?;
    Ok(inout)
}

fn encode_compress_entries<'a: 'b, 'b>(
    entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>>,
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
    let it = decode_entries::<PlaintextBytes, Timestamp>(&out);
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
    async fn init_plaintext<'a: 'b, 'b>(
        signature: Signature,
        prev_chain: ChainHash,
        start_cipher: Option<CipherInfo>,
        log_id: &LogId,
        key_resolver: &dyn CipherKeyResolver,
        entries: impl Iterator<Item = &'b PlaintextLogEntry<'a>> + Clone,
    ) -> Result<(Self, Option<(SegmentCipher, Vec<u8>)>), SegmentEncodeError> {
        let mut end_cipher = start_cipher;
        let mut header = Self::init_opaque(
            signature,
            prev_chain,
            start_cipher,
            entries.clone().next().as_ref(),
        );

        for e in entries {
            if let LogEntry::IndexedEntry(EntryBody::UseKey(ci)) = e {
                end_cipher = Some(*ci);
            }
        }

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
            let segment_cipher = SegmentCipher::new(cipher_suite, key, &log_id.peer_id);

            // we only store the cipher here if it is different from what is recorded in start_cipher
            // otherwise we can just use what's in start cipher
            let cipher = if end_cipher == header.start_cipher {
                None
            } else {
                Some(ci)
            };

            (
                Some(EncryptionInfo {
                    cipher,
                    nonce: nonce.clone(),
                }),
                Some((segment_cipher, nonce)),
            )
        } else {
            (None, None)
        };
        header.encoding = SegmentEncoding::Plaintext(PlaintextSegmentEncoding {
            outer_encryption: enc,
            inner_compression: Compression::Zstd,
        });

        Ok((header, cipher))
    }

    fn init_opaque<B: BytesWrapper, T: TimestampRepr>(
        signature: Signature,
        prev_chain: ChainHash,
        mut start_cipher: Option<CipherInfo>,
        first_entry: Option<&&LogEntry<B, T>>,
    ) -> Self {
        // if the first entry is a UseKey entry, no point in encoding start_cipher
        if let Some(LogEntry::IndexedEntry(EntryBody::UseKey(_))) = first_entry {
            start_cipher = None;
        }
        Self {
            prev_chain,
            start_cipher,
            signature,
            encoding: SegmentEncoding::Opaque,
        }
    }

    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        self.prev_chain.encode(w);
        self.signature.encode(w)?;
        self.encoding.encode(w)?;
        w.write_option(&self.start_cipher, |w, c| c.encode(w))?;
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let prev_chain = ChainHash::decode(r)?;
        let signature = Signature::decode(r).map_err(SegmentDecodeError::from_sig_decode_err)?;
        let encoding = SegmentEncoding::decode(r)?;
        let start_cipher = r.read_option(|r| CipherInfo::decode(r))?;
        Ok(Self {
            signature,
            encoding,
            prev_chain,
            start_cipher,
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
                enc.encode(w)?;
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
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_option(&self.outer_encryption, |w, e| e.encode(w))?;
        w.write_byte(self.inner_compression.into());
        Ok(())
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
    pub fn encode(&self, w: &mut Writer) -> Result<(), WriteError> {
        w.write_option(&self.cipher, |w, c| c.encode(w))?;
        w.write_len_prefixed(&self.nonce);
        Ok(())
    }

    pub fn decode(r: &mut Reader) -> Result<Self, SegmentDecodeError> {
        let cipher = r.read_option(|r| CipherInfo::decode(r))?;
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

impl<'a> DecodedSegment<'a> {
    pub fn verify(&self, verifying_key: &VerifyingKey) -> Result<(), SegmentVerifyError> {
        verifying_key.verify_signature(
            &self.head_chain.sign_bytes(&self.chain_seed),
            &self.header.signature,
        )?;
        Ok(())
    }

    pub async fn to_plaintext(
        self,
        key_resolver: &dyn CipherKeyResolver,
    ) -> DecodedPlaintextEntries<'a> {
        let header = self.header;
        let mut active_cipher = header.start_cipher;
        let mut entry_cipher = if let Some(ci) = active_cipher {
            Some(
                match EntryCipher::resolve(&ci, self.chain_seed.log_id(), key_resolver).await {
                    Ok(c) => c,
                    Err(e) => {
                        return DecodedPlaintextEntries {
                            entries: vec![Err(e.into())],
                        };
                    }
                },
            )
        } else {
            None
        };
        let mut entries = vec![];
        let mut prev_chain = header.prev_chain;
        for e in self.entries {
            match e.body {
                DecodedEntryBody::Opaque(b) => {
                    let mut next = async || {
                        let b2 = to_plaintext(&b, &entry_cipher, &prev_chain)?;
                        prev_chain = e.chain_meta.chain_hash;
                        if let LogEntry::IndexedEntry(EntryBody::UseKey(cipher_info)) = b2 {
                            active_cipher = Some(cipher_info);
                        };
                        check_cipher_change(
                            &active_cipher,
                            &mut entry_cipher,
                            &self.chain_seed,
                            key_resolver,
                        )
                        .await?;
                        Ok((b2, e.chain_meta))
                    };
                    if entries.push_mut(next().await).is_err() {
                        break;
                    }
                }
                DecodedEntryBody::Plaintext(b) => entries.push(Ok((b, e.chain_meta))),
            }
        }
        DecodedPlaintextEntries { entries }
    }

    /// Iterates through all `ChainMeta` items in the log starting with `prev_chain`,
    ///  skipping duplicates.
    pub fn chain_meta_iter(&self, start_size: u64) -> impl Iterator<Item = ChainMeta> {
        std::iter::once(ChainMeta {
            chain_hash: self.header.prev_chain,
            cipher_info: self.header.start_cipher,
        })
        .chain(self.entries.iter().map(|e| e.chain_meta))
        .dedup()
        .skip_while(move |m| m.chain_hash.size < start_size)
    }

    pub async fn reencode_suffix(
        &self,
        key_resolver: &dyn CipherKeyResolver,
        start_size: u64,
    ) -> Result<Vec<u8>, SegmentEncodeError> {
        let Some(ChainMeta {
            cipher_info: start_cipher,
            chain_hash: prev_chain,
        }) = self.chain_meta_iter(start_size).next()
        else {
            return Err(SegmentEncodeError::StartSizeOutOfRange(start_size));
        };
        // collected up front to avoid Send bound errors with futures
        let entries: Vec<&DecodedEntry<'_>> = self
            .entries
            .iter()
            .filter(|e| e.chain_meta.chain_hash.size > start_size)
            .collect();
        let Some(first) = entries.first() else {
            return Err(SegmentEncodeError::StartSizeOutOfRange(start_size));
        };
        // we assume that all entry bodies have the same encoding because that's how we decoded them
        match first.body {
            DecodedEntryBody::Opaque(_) => encode_segment_opaque(
                &self.header.signature,
                &prev_chain,
                &start_cipher,
                entries.iter().map(|e| match e.body {
                    DecodedEntryBody::Opaque(ref e) => e,
                    DecodedEntryBody::Plaintext(_) => {
                        unreachable!("entries should be all opaque or plaintext, not mixed")
                    }
                }),
            ),
            DecodedEntryBody::Plaintext(_) => {
                let bodies: Vec<&PlaintextLogEntry<'_>> = entries
                    .iter()
                    .map(|e| match e.body {
                        DecodedEntryBody::Plaintext(ref e) => e,
                        DecodedEntryBody::Opaque(_) => {
                            unreachable!("entries should be all opaque or plaintext, not mixed")
                        }
                    })
                    .collect();
                encode_segment_plaintext(
                    &self.header.signature,
                    &prev_chain,
                    &start_cipher,
                    self.chain_seed.log_id(),
                    key_resolver,
                    bodies.iter().copied(),
                )
                .await
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainMeta {
    pub chain_hash: ChainHash,
    pub cipher_info: Option<CipherInfo>,
}

impl<'a> ToStatic for DecodedSegment<'a> {
    type Static = DecodedSegment<'static>;

    fn to_static(self) -> Self::Static {
        Self::Static {
            header: self.header,
            chain_seed: self.chain_seed,
            entries: self.entries.into_iter().map(|e| e.to_static()).collect(),
            head_chain: self.head_chain,
            head_cipher: self.head_cipher,
        }
    }
}

impl<'a> ToStatic for DecodedEntry<'a> {
    type Static = DecodedEntry<'static>;

    fn to_static(self) -> Self::Static {
        Self::Static {
            chain_meta: self.chain_meta,
            body: self.body.to_static(),
        }
    }
}

impl<'a> ToStatic for DecodedEntryBody<'a> {
    type Static = DecodedEntryBody<'static>;

    fn to_static(self) -> Self::Static {
        match self {
            DecodedEntryBody::Opaque(e) => DecodedEntryBody::Opaque(e.to_static()),
            DecodedEntryBody::Plaintext(e) => DecodedEntryBody::Plaintext(e.to_static()),
        }
    }
}

pub struct DecodedPlaintextEntries<'a> {
    pub entries: Vec<Result<(PlaintextLogEntry<'a>, ChainMeta), SegmentCipherError>>,
}

impl<'a> DecodedPlaintextEntries<'a> {
    pub fn collect_all(self) -> Result<Vec<PlaintextLogEntry<'a>>, SegmentCipherError> {
        let mut res = vec![];
        for e in self.entries {
            let (e, _) = e?;
            res.push(e);
        }
        Ok(res)
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
        hlc::Timestamp,
        ids::{ContainerId, LogId},
        log::{
            ChainHash, EntryBody, LogEntry, OpEntry, PlaintextLogEntry, entries_to_opaque,
            segment::{encode_segment_opaque, encode_segment_plaintext},
        },
    };

    use crate::log::{LogHashContext, segment::SegmentReader};

    #[derive(Debug, Arbitrary)]
    pub(crate) enum TestEntry {
        #[weight(5)]
        Ops(OpEntry<PlaintextBytes<'static>, Timestamp>),
        #[weight(2)]
        UseKey([u8; 32]),
        #[weight(1)]
        Signature,
    }

    #[derive(Debug, Arbitrary)]
    pub(crate) struct TestCase {
        pub start_key: Option<[u8; 32]>,
        pub entries: Vec<TestEntry>,
        pub signing_key: [u8; 32],
        pub log_id: LogId,
        pub prev_chain: ChainHash,
    }

    pub(crate) struct TestCaseData {
        pub case: TestCase,
        pub start_cipher: Option<CipherInfo>,
        pub end_cipher: Option<CipherInfo>,
        pub prev_chain: ChainHash,
        pub head_chain: ChainHash,
        pub entries: Vec<PlaintextLogEntry<'static>>,
        pub key_resolver: TestKeyResolver,
        pub signature: Signature,
        pub verifying_key: VerifyingKey,
        pub seed: LogHashContext,
    }

    pub(crate) type TestKeyResolver = HashMap<RootKey256Fingerprint, RootKey256>;

    impl TestCase {
        pub(crate) async fn data(self) -> TestCaseData {
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
            let seed = LogHashContext::new(&self.log_id);
            for e in self.entries.iter() {
                let e = match e {
                    TestEntry::Ops(op) => LogEntry::IndexedEntry(EntryBody::Op(op.clone())),
                    TestEntry::UseKey(k) => {
                        LogEntry::IndexedEntry(EntryBody::UseKey(switch_key(*k, &mut key_resolver)))
                    }
                    TestEntry::Signature => LogEntry::Signature(
                        signing_key.sign(&head_chain.sign_bytes(&seed)).unwrap(),
                    ),
                };
                let last_meta = entries_to_opaque(
                    &seed,
                    &mut head_cipher,
                    &head_chain,
                    &key_resolver,
                    [e.clone()].iter(),
                )
                .await
                .last()
                .unwrap()
                .clone()
                .unwrap()
                .1;
                head_chain = last_meta.chain_hash;
                entries.push(e);
            }
            TestCaseData {
                case: self,
                start_cipher,
                end_cipher: head_cipher,
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
            &data.case.log_id,
            &data.key_resolver,
            data.entries.iter(),
        )
        .await
        .unwrap();
        check_decode(&plaintext_segment, &data).await;

        let mut head_cipher = data.start_cipher;
        let opaque: Result<Vec<_>, _> = entries_to_opaque(
            &data.seed,
            &mut head_cipher,
            &data.prev_chain,
            &data.key_resolver,
            data.entries.iter(),
        )
        .await
        .into_iter()
        .collect();
        let opaque = opaque.unwrap();
        assert_eq!(data.end_cipher, head_cipher);
        // this assertion is weirdly tolerant of empty segments which we maybe should reject, but currently they're sort of benign
        assert_eq!(
            data.head_chain,
            opaque
                .last()
                .map(|e| e.1.chain_hash)
                .unwrap_or(data.prev_chain)
        );

        let opaque_segment = encode_segment_opaque(
            &data.signature,
            &data.prev_chain,
            &data.start_cipher,
            opaque.iter().map(|(e, _)| e),
        )
        .unwrap();
        check_decode(&opaque_segment, &data).await;
    }

    async fn check_decode(segment: &[u8], data: &TestCaseData) {
        let reader = SegmentReader::start(segment).unwrap();
        let decoded = reader.read(&data.key_resolver, &data.seed).await.unwrap();
        decoded.verify(&data.verifying_key).unwrap();
        assert_eq!(data.prev_chain, decoded.header.prev_chain);
        assert_eq!(data.head_chain, decoded.head_chain);
        assert_eq!(data.end_cipher, decoded.head_cipher);
        let decoded_plaintext = decoded
            .to_plaintext(&data.key_resolver)
            .await
            .collect_all()
            .unwrap();

        assert_eq!(data.entries, decoded_plaintext);
    }
}
