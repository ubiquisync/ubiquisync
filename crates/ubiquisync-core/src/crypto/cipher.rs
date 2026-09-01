use std::borrow::Borrow;
use std::ops::Range;

use chacha20::ChaCha20;
use chacha20::KeyIvInit;
use chacha20::cipher::StreamCipher;
use chacha20poly1305::AeadInOut;
use chacha20poly1305::ChaCha20Poly1305;
use chacha20poly1305::Nonce;
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

use crate::bytes::OpaqueBytes;
use crate::bytes::PlaintextBytes;
use crate::codec::ReadError;
use crate::codec::Reader;
use crate::codec::Writer;
use crate::crypto::Hash256;
use crate::ids::ContainerId;
use crate::ids::LogId;

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
///    Only an extremely rare clock issue could cause the header bytes to be identical
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
    ChaCha20Poly1305 = 0,
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
}

const DERIVE_PREFIX_LEN: usize = 1 + 32 + 32 + 16;

struct CipherBase {
    key: ContainerKey256,
    // TODO: instead of allocating a Vec can we just have a Hasher here that we clone when needed?
    derive_prefix: [u8; DERIVE_PREFIX_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct RootKey256Fingerprint(pub [u8; 32]);

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl CipherBase {
    fn new(suite: u8, key: ContainerKey256, log_id: &LogId) -> Self {
        let mut derive_prefix = [0; DERIVE_PREFIX_LEN];
        derive_prefix[0] = suite;
        derive_prefix[1..33].copy_from_slice(&key.root_fingerprint.0[..]);
        derive_prefix[33..65].copy_from_slice(&log_id.peer_id.0[..]);
        derive_prefix[65..].copy_from_slice(&log_id.container_id.0[..]); // TODO we still need container ID if using container scoped keys?
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
        kdf.update(&[KDF_DOMAIN_LOG_ENTRY.len() as u8]); // TODO we do we even need this len here?
        kdf.update(KDF_DOMAIN_LOG_ENTRY.as_bytes());
        kdf.update(&base.derive_prefix);
        Self { kdf, base }
    }

    pub fn slot_cipher(&self, entry_idx: u64) -> SlotCipher {
        let mut kdf = self.kdf.clone();
        kdf.update(&entry_idx.to_le_bytes());
        SlotCipher { kdf, slot_index: 0 }
    }

    // pub fn encrypt_header(
    //     &self,
    //     entry_idx: u64,
    //     prev_hash: &Hash256,
    //     header: &PlaintextBytes,
    // ) -> Result<OpaqueBytes<'static>, CipherError> {
    //     Ok(self
    //         .cipher_slot(entry_idx, 0, prev_hash, header.borrow())
    //         .into())
    // }

    // pub fn encrypt_op(
    //     &self,
    //     entry_idx: u64,
    //     op_index: u64,
    //     prev_hash: &Hash256,
    //     op: &PlaintextBytes,
    // ) -> Result<OpaqueBytes<'static>, CipherError> {
    //     Ok(self
    //         .cipher_slot(
    //             entry_idx,
    //             op_index.checked_add(1).expect("op index overflow"),
    //             prev_hash,
    //             op.borrow(),
    //         )
    //         .into()) // convert to 1-based index, 0 for header
    // }

    // fn derive_key(&self, entry_idx: u64, slot_idx: u64, prev_hash: &Hash256) -> ChaCha20 {
    //     let kdf = self.kdf.clone();
    //     kdf.update(data);
    //     key_info.extend_from_slice(&entry_idx.to_le_bytes());
    //     key_info.extend_from_slice(&slot_idx.to_le_bytes());
    //     key_info.extend_from_slice(&prev_hash[..]);
    //     let key = derive_key(RootKeyDomain::EntryCipher, &self.base.key, &key_info);
    //     let nonce = [0; 12]; // since we derive the key we can use a zero nonce
    //     ChaCha20::new(key.0.expose_secret().into(), &nonce.into())
    // }

    // pub fn decrypt_header(
    //     &self,
    //     entry_idx: u64,
    //     prev_hash: &Hash256,
    //     header: &OpaqueBytes,
    // ) -> Result<PlaintextBytes<'static>, CipherError> {
    //     Ok(self
    //         .cipher_slot(entry_idx, 0, prev_hash, header.borrow())
    //         .into())
    // }

    // pub fn decrypt_op(
    //     &self,
    //     entry_idx: u64,
    //     op_index: u64,
    //     prev_hash: &Hash256,
    //     op: &OpaqueBytes,
    // ) -> Result<PlaintextBytes<'static>, CipherError> {
    //     Ok(self
    //         .cipher_slot(
    //             entry_idx,
    //             op_index.checked_add(1).expect("op index overflow"),
    //             prev_hash,
    //             op.borrow(),
    //         )
    //         .into()) // convert to 1-based index, 0 for header
    // }

    // fn cipher_slot(
    //     &self,
    //     entry_idx: u64,
    //     slot_idx: u64,
    //     prev_hash: &Hash256,
    //     bytes: &[u8],
    // ) -> Vec<u8> {
    //     let mut cipher = self.derive_key(entry_idx, slot_idx, prev_hash);
    //     let mut res = Vec::from(bytes); // we copy the input data to the res vec to decrypt in place
    //     cipher.apply_keystream(res.as_mut_slice());
    //     res
    // }

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
        kdf.update(&[1u8]);
        // output is Array which should implement zeroize already
        let okm = kdf.finalize_fixed();
        let mut cipher = ChaCha20::new(&okm, &[0; 12].into());
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
            SegmentCipherSuite::ChaCha20Poly1305,
            "if this gets triggered it means we need to support new cipher suites",
        );

        let base = CipherBase::new(suite.into(), key, log_id);

        Self { base }
    }

    pub fn decrypt_segment(
        &self,
        range: &Range<u64>,
        nonce: &[u8; 16],
        inout: &mut Vec<u8>,
    ) -> Result<(), CipherError> {
        let (ad, cipher, nonce) = self.segment_ad_and_cipher(range, nonce);
        cipher
            .decrypt_in_place(&nonce, &ad, inout)
            .map_err(|_| CipherError)?;
        Ok(())
    }

    pub fn encrypt_segment(
        &self,
        range: &Range<u64>,
        nonce: &[u8; 16],
        inout: &mut Vec<u8>,
    ) -> Result<(), CipherError> {
        let (ad, cipher, nonce) = self.segment_ad_and_cipher(range, nonce);
        cipher
            .encrypt_in_place(&nonce, &ad, inout)
            .map_err(|_| CipherError)?;
        Ok(())
    }

    fn segment_ad_and_cipher(
        &self,
        range: &Range<u64>,
        nonce: &[u8; 16],
    ) -> (Vec<u8>, ChaCha20Poly1305, Nonce) {
        let mut ad = Vec::new();
        ad.extend_from_slice(self.base.derive_prefix.as_slice());
        ad.extend_from_slice(&range.start.to_le_bytes());
        ad.extend_from_slice(&range.end.to_le_bytes());
        // TODO do we need end in AD too?
        let mut key_info = ad.clone();
        key_info.extend_from_slice(nonce);
        let mut key = [0; 32]; // TODO this probably shouldn't be on stack
        kdf(
            KDF_DOMAIN_LOG_SEGMENT,
            &self.base.key.key,
            &key_info,
            &mut key,
        );
        let cipher = ChaCha20Poly1305::new(&key.into());
        (ad, cipher, [0; 12].into()) // since we derive every key based on coordinates, we can use a zero nonce
    }

    pub fn key_fingerprint(&self) -> &RootKey256Fingerprint {
        &self.base.key.root_fingerprint
    }

    pub fn cipher_suite(&self) -> SegmentCipherSuite {
        SegmentCipherSuite::ChaCha20Poly1305
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
