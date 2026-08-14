use aes_gcm_siv::AeadInOut;
use aes_gcm_siv::Aes256GcmSiv;
use aes_gcm_siv::KeyInit;
use aes_gcm_siv::Nonce;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::ExposeSecret;
use secrecy::SecretBox;
use thiserror::Error;
use zeroize::Zeroize;
use zeroize::ZeroizeOnDrop;

use crate::crypto::Hash;
use crate::ids::ContainerId;
use crate::ids::PeerId;

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CipherSuite {
    Aes256GcmSiv = 0,
}

pub struct EntryCipher {
    cipher: Aes256GcmSiv,
    ad_prefix: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Key256(SecretBox<[u8; 32]>);

impl Key256 {
    // TODO method to convert from existing bytes and scrub them on conversion
    // TODO method to securely generate key

    pub fn fingerprint(&self) -> [u8; 32] {
        blake3::derive_key(DOMAIN_KEY_FINGERPRINT, self.0.expose_secret())
    }

    pub fn cipher(&self) -> Aes256GcmSiv {
        Aes256GcmSiv::new(self.0.expose_secret().into())
    }
}

const DOMAIN_KEY_FINGERPRINT: &str = "ubiquisync/v1/key-fingerprint";
const DOMAIN_AEAD_NONCE: &str = "ubiquisync/v1/aead-nonce";

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl EntryCipher {
    pub fn new(key: Key256, peer_id: &PeerId, container_id: &ContainerId) -> Self {
        let mut ad_prefix = vec![];
        ad_prefix.push(CipherSuite::Aes256GcmSiv.into());
        ad_prefix.extend_from_slice(&key.fingerprint()[..]);
        ad_prefix.extend_from_slice(&peer_id.0[..]);
        ad_prefix.extend_from_slice(&container_id.0[..]);
        Self {
            ad_prefix,
            cipher: key.cipher(),
        }
    }

    pub fn encrypt_header(&self, entry_idx: u64, header: &[u8]) -> Result<Vec<u8>, CipherError> {
        self.encrypt_slot(entry_idx, 0, header)
    }

    pub fn encrypt_op(
        &self,
        entry_idx: u64,
        op_index: u64,
        op: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        self.encrypt_slot(
            entry_idx,
            op_index.checked_add(1).expect("op index overflow"),
            op,
        ) // convert to 1-based index, 0 for header
    }

    fn encrypt_slot(
        &self,
        entry_idx: u64,
        slot_idx: u64,
        bytes: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let (ad, nonce) = self.associated_data_and_nonce(entry_idx, slot_idx);
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to encrypt in place
        self.cipher
            .encrypt_in_place(&nonce, &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res)
    }

    fn associated_data_and_nonce(&self, entry_idx: u64, slot_idx: u64) -> (Vec<u8>, Nonce) {
        // TODO derive subkey
        let mut ad = Vec::new();
        const AD_LEN: usize = 1 + 32 + 16 + 16 + 8 + 8;
        ad.reserve_exact(AD_LEN);
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&entry_idx.to_le_bytes());
        ad.extend_from_slice(&slot_idx.to_le_bytes());
        let nonce: [u8; 12] = blake3::derive_key(DOMAIN_AEAD_NONCE, &ad[..])[0..12]
            .try_into()
            .unwrap();
        (ad, nonce.into())
    }

    pub fn decrypt_header(&self, entry_idx: u64, header: &[u8]) -> Result<Vec<u8>, CipherError> {
        self.decrypt_slot(entry_idx, 0, header)
    }

    pub fn decrypt_op(
        &self,
        entry_idx: u64,
        op_index: u64,
        op: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        self.decrypt_slot(
            entry_idx,
            op_index.checked_add(1).expect("op index overflow"),
            op,
        ) // convert to 1-based index, 0 for header
    }

    fn decrypt_slot(
        &self,
        entry_idx: u64,
        slot_idx: u64,
        bytes: &[u8],
    ) -> Result<Vec<u8>, CipherError> {
        let (ad, nonce) = self.associated_data_and_nonce(entry_idx, slot_idx);
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to decrypt in place
        self.cipher
            .decrypt_in_place(&nonce, &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res)
    }
}

pub struct EncryptionKeyRing {}

impl EncryptionKeyRing {
    pub fn get_key(&self, fingerprint: &Hash) -> Option<Key256> {
        todo!()
    }
}
