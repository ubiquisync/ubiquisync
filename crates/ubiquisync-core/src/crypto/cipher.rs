use std::borrow::Borrow;

use chacha20::ChaCha20;
use chacha20::KeyIvInit;
use chacha20::cipher::StreamCipher;
use chacha20poly1305::AeadInOut;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::XNonce;
use crypto_common::KeyInit;
use hkdf::Hkdf;
use hmac::Hmac;
use hmac::Mac;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::ExposeSecret;
use secrecy::SecretBox;
use sha2::Sha256;
use sha2::digest::FixedOutput;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::bytes::OpaqueBytes;
use crate::bytes::PlaintextBytes;
use crate::codec::ReadError;
use crate::codec::Reader;
use crate::codec::Writer;
use crate::crypto::Hash256;
use crate::ids::ContainerId;
use crate::ids::LogId;
use crate::log::ChainHash;

/// The cipher suite for per-entry encryption used for canonical entry hashing
/// for signatures. This mode is only used for transport and storage when the
/// receiving party does not need to decrypt the data such as a blind relay.
/// For this use case we use a non-AEAD cipher because:
/// 1. All data is verified by signatures anyway so we don't care about authentication
/// 2. We derive sub-keys for misuse resistance and the only edge case where an honest
///    peer can realistically reuse a sub-key is in the first header of a fork.
///    As long as the timestamp of the header is different from the timestamp at that
///    entry before the fork, the sub-key used for the actual entry body will be distinct
///    because it is derived from the encrypted hash of that header.
///    Only a rare clock issue could cause the header bytes to be identical
///    or intentional misuse.
/// 3. For blind relays which can't compress entries, tag overhead (32 bytes/entry)
///    is relatively large compared to many realistic entry payloads (ex. keystroke edits).
#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EntryCipherSuite {
    ChaCha20 = 0,
}

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum SegmentCipherSuite {
    XChaCha20Poly1305 = 0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct CipherInfo {
    /// The raw decoded cipher suite. We retain unknown cipher suites to indicate
    /// that the ciphertext may be from a newer client using a cipher suite we
    /// don't know about - such a scenario would indicate a software upgrade may
    /// be needed.
    pub cipher_suite: u8,
    pub fingerprint: RootKey256Fingerprint,
}

pub struct EntryCipher {
    base: CipherBase,
    kdf: Hmac<Sha256>,
}

pub struct SegmentCipher {
    base: CipherBase,
    cipher: XChaCha20Poly1305,
}

const DERIVE_PREFIX_LEN: usize = 1 + 32 + 32 + 16;

struct CipherBase {
    key: ContainerKey256,
    derive_prefix: [u8; DERIVE_PREFIX_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct RootKey256Fingerprint(pub [u8; 32]);

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl SegmentCipherSuite {
    pub fn nonce_size(&self) -> usize {
        match self {
            SegmentCipherSuite::XChaCha20Poly1305 => 24,
        }
    }
}

impl CipherBase {
    fn new(suite: u8, key: ContainerKey256, log_id: &LogId) -> Self {
        let mut derive_prefix = [0; DERIVE_PREFIX_LEN];
        derive_prefix[0] = suite;
        derive_prefix[1..33].copy_from_slice(&key.root_fingerprint.0[..]);
        derive_prefix[33..65].copy_from_slice(&log_id.peer_id.0[..]);
        derive_prefix[65..].copy_from_slice(&log_id.container_id.0[..]);
        Self { key, derive_prefix }
    }
}

pub struct RootKey256 {
    fingerprint: RootKey256Fingerprint,
    key: SecretBox<[u8; 32]>,
}

fn domain_len(domain: &str) -> u8 {
    domain
        .len()
        .try_into()
        .expect("domain string should have len <= 255")
}

fn kdf(domain: &str, key: &SecretBox<[u8; 32]>, info: &[u8], output: &mut [u8; 32]) {
    let len: u8 = domain_len(domain);
    let hk = Hkdf::<Sha256>::new(None, key.expose_secret());
    hk.expand_multi_info(&[&[len], domain.as_bytes(), info], output)
        .expect("valid lengths");
}

impl RootKey256 {
    pub fn new(key: SecretBox<[u8; 32]>) -> Self {
        let mut fingerprint = [0; 32];
        kdf(KDF_DOMAIN_ENC_KEY_FINGERPRINT, &key, &[], &mut fingerprint);
        Self {
            key,
            fingerprint: RootKey256Fingerprint(fingerprint),
        }
    }

    pub fn container_key(&self, container_id: &ContainerId) -> ContainerKey256 {
        let key = SecretBox::<[u8; 32]>::init_with_mut(|output| {
            kdf(KDF_DOMAIN_CONTAINER, &self.key, &container_id.0, output)
        });
        ContainerKey256 {
            root_fingerprint: self.fingerprint,
            key,
        }
    }
}

/// We always use per-container derived keys before any other key derivation so
/// that a workspace could have a security policy where a single key is used
/// to encrypt multiple containers which all have the same security audience,
/// but which could allow them to rotate the keys for a subset of the containers
/// when the audience/container alignment changes. When rotating encryption keys,
/// it would be standard practice to encrypt the old encryption key with the new key.
/// But if the key is shared across containers and container membership changes
/// we don't want to leak the root key which covered all containers.
/// Instead we can safely share per-contained derived sub-keys without leaking
/// the root key.
pub struct ContainerKey256 {
    root_fingerprint: RootKey256Fingerprint,
    key: SecretBox<[u8; 32]>,
}

impl ContainerKey256 {
    pub fn root_fingerprint(&self) -> &RootKey256Fingerprint {
        &self.root_fingerprint
    }
}

const KDF_DOMAIN_ENC_KEY_FINGERPRINT: &str = "ubq/v1/kdf/EncFingerprint";
const KDF_DOMAIN_CONTAINER: &str = "ubq/v1/kdf/Container";
const KDF_DOMAIN_LOG_ENTRY: &str = "ubq/v1/kdf/LogEntry";
const KDF_DOMAIN_LOG_SEGMENT: &str = "ubq/v1/kdf/LogSegment";

pub struct SlotCipher {
    kdf: Hmac<Sha256>,
    slot_index: u64,
}

impl EntryCipher {
    pub fn new(suite: EntryCipherSuite, key: ContainerKey256, log_id: &LogId) -> Self {
        assert_eq!(
            suite,
            EntryCipherSuite::ChaCha20,
            "if this gets triggered it means we need to support new cipher suites",
        );
        let mut kdf = <Hmac<Sha256> as KeyInit>::new_from_slice(key.key.expose_secret())
            .expect("32 byte key");
        let base = CipherBase::new(suite.into(), key, log_id);
        kdf.update(&[KDF_DOMAIN_LOG_ENTRY.len() as u8]);
        kdf.update(KDF_DOMAIN_LOG_ENTRY.as_bytes());
        kdf.update(&base.derive_prefix);
        Self { kdf, base }
    }

    pub fn slot_cipher(&self, entry_idx: u64) -> SlotCipher {
        let mut kdf = self.kdf.clone();
        kdf.update(&entry_idx.to_le_bytes());
        SlotCipher { kdf, slot_index: 0 }
    }

    pub fn key_fingerprint(&self) -> &RootKey256Fingerprint {
        &self.base.key.root_fingerprint
    }

    pub fn cipher_suite(&self) -> EntryCipherSuite {
        EntryCipherSuite::ChaCha20
    }

    pub fn cipher_info(&self) -> CipherInfo {
        CipherInfo {
            cipher_suite: self.cipher_suite().into(),
            fingerprint: self.base.key.root_fingerprint,
        }
    }
}

impl SlotCipher {
    fn cipher_slot_in_place(&mut self, prev_hash: &Hash256, buf: &mut [u8]) {
        let mut kdf = self.kdf.clone();
        let slot_index = self.slot_index;
        self.slot_index += 1;
        kdf.update(&slot_index.to_le_bytes());
        kdf.update(prev_hash);
        kdf.update(&[1u8]); // this results in essentially the same behavior as HKDF extract-only
        let okm: Zeroizing<[u8; 32]> = Zeroizing::new(kdf.finalize_fixed().into());
        let mut cipher = ChaCha20::new((&*okm).into(), &[0; 12].into());
        cipher.apply_keystream(buf);
    }

    fn cipher_slot(&mut self, prev_hash: &Hash256, bytes: &[u8]) -> Vec<u8> {
        let mut res = Vec::from(bytes);
        self.cipher_slot_in_place(prev_hash, res.as_mut_slice());
        res
    }

    pub fn encrypt_slot(
        &mut self,
        prev_hash: &Hash256,
        bytes: &PlaintextBytes,
    ) -> Result<OpaqueBytes<'static>, CipherError> {
        Ok(self.cipher_slot(prev_hash, bytes.borrow()).into())
    }

    /// Advances the slot index without doing any encryption/decryption. ONLY to be used when skipping over expunged slots.
    pub fn skip_slot(&mut self) {
        self.slot_index += 1;
    }

    pub fn decrypt_slot(
        &mut self,
        prev_hash: &Hash256,
        bytes: &OpaqueBytes,
    ) -> Result<PlaintextBytes<'static>, CipherError> {
        Ok(self.cipher_slot(prev_hash, bytes.borrow()).into())
    }
}

impl SegmentCipher {
    pub fn new(suite: SegmentCipherSuite, key: ContainerKey256, log_id: &LogId) -> Self {
        assert_eq!(
            suite,
            SegmentCipherSuite::XChaCha20Poly1305,
            "if this gets triggered it means we need to support new cipher suites",
        );

        let base = CipherBase::new(suite.into(), key, log_id);
        let mut key = Zeroizing::new([0; 32]);
        kdf(KDF_DOMAIN_LOG_SEGMENT, &base.key.key, &[], &mut key);
        let cipher = XChaCha20Poly1305::new((&*key).into());
        Self { base, cipher }
    }

    pub fn decrypt_segment(
        &self,
        prev_chain: &ChainHash,
        count: u64,
        nonce: &[u8],
        inout: &mut Vec<u8>,
    ) -> Result<(), CipherError> {
        let (ad, nonce) = self.segment_ad_and_cipher(prev_chain, count, nonce)?;
        self.cipher
            .decrypt_in_place(&nonce, &ad, inout)
            .map_err(|_| CipherError)?;
        Ok(())
    }

    pub fn encrypt_segment(
        &self,
        prev_chain: &ChainHash,
        count: u64,
        nonce: &[u8],
        inout: &mut Vec<u8>,
    ) -> Result<(), CipherError> {
        let (ad, nonce) = self.segment_ad_and_cipher(prev_chain, count, nonce)?;
        self.cipher
            .encrypt_in_place(&nonce, &ad, inout)
            .map_err(|_| CipherError)?;
        Ok(())
    }

    fn segment_ad_and_cipher(
        &self,
        prev_chain: &ChainHash,
        count: u64,
        nonce: &[u8],
    ) -> Result<(Vec<u8>, XNonce), CipherError> {
        let mut ad = Vec::new();
        ad.extend_from_slice(self.base.derive_prefix.as_slice());
        ad.extend_from_slice(&prev_chain.hash);
        ad.extend_from_slice(&prev_chain.size.to_le_bytes());
        ad.extend_from_slice(&count.to_le_bytes());
        let xnonce = nonce.try_into().map_err(|_| CipherError)?;
        Ok((ad, xnonce))
    }

    pub fn key_fingerprint(&self) -> &RootKey256Fingerprint {
        &self.base.key.root_fingerprint
    }

    pub fn cipher_suite(&self) -> SegmentCipherSuite {
        SegmentCipherSuite::XChaCha20Poly1305
    }

    pub fn cipher_info(&self) -> CipherInfo {
        CipherInfo {
            cipher_suite: self.cipher_suite().into(),
            fingerprint: self.base.key.root_fingerprint,
        }
    }
}

impl CipherInfo {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte(self.cipher_suite);
        writer.write_array(&self.fingerprint.0);
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        let cipher_suite = reader.read_byte()?;
        Ok(CipherInfo {
            cipher_suite,
            fingerprint: RootKey256Fingerprint(reader.read_array()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use test_strategy::proptest;

    use crate::bytes::PlaintextBytes;
    use crate::crypto::RootKey256;
    use crate::ids::{ContainerId, LogId};
    use crate::{
        crypto::{EntryCipher, EntryCipherSuite},
        ids::PeerId,
    };

    #[proptest]
    fn test_roundtrip(
        key: [u8; 32],
        peer_id: [u8; 32],
        container_id: [u8; 16],
        last_hash: [u8; 32],
        entry_idx: u64,
        slots: Vec<PlaintextBytes<'static>>,
    ) {
        let key = RootKey256::new(SecretBox::new(Box::new(key)));
        let log_id = LogId {
            peer_id: PeerId(peer_id),
            container_id: ContainerId(container_id),
        };
        let container_key = key.container_key(&log_id.container_id);
        let cipher = EntryCipher::new(EntryCipherSuite::ChaCha20, container_key, &log_id);
        let mut slot_cipher = cipher.slot_cipher(entry_idx);
        let mut encrypted = vec![];
        for slot in slots.iter() {
            encrypted.push(slot_cipher.encrypt_slot(&last_hash, slot).unwrap())
        }
        let mut slot_cipher = cipher.slot_cipher(entry_idx);
        let mut decrypted = vec![];
        for slot in encrypted.iter() {
            decrypted.push(slot_cipher.decrypt_slot(&last_hash, slot).unwrap())
        }
        assert_eq!(slots, decrypted);
    }

    // TODO snapshot tests
}
