use aes_gcm_siv::AeadInOut;
use aes_gcm_siv::Aes256GcmSiv;
use aes_gcm_siv::KeyInit;
use aes_gcm_siv::Nonce;
use blake3::Hasher;
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
    key_hasher: Hasher,
    ad_prefix: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Key256(pub SecretBox<[u8; 32]>);

impl Key256 {
    pub fn fingerprint(&self) -> [u8; 32] {
        blake3::derive_key(DOMAIN_KEY_FINGERPRINT, self.0.expose_secret())
    }
}

const DOMAIN_KEY_FINGERPRINT: &str = "ubiquisync/v1/key-fingerprint";
const DOMAIN_DERIVE_KEY: &str = "ubiquisync/v1/derive-key";

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl EntryCipher {
    pub fn new(
        suite: CipherSuite,
        key: Key256,
        peer_id: &PeerId,
        container_id: &ContainerId,
    ) -> Self {
        assert_eq!(
            suite,
            CipherSuite::Aes256GcmSiv,
            "if this gets triggered it means we need to support new cipher suites",
        );

        let mut ad_prefix = vec![];
        ad_prefix.push(suite.into());
        ad_prefix.extend_from_slice(&key.fingerprint()[..]);
        ad_prefix.extend_from_slice(&peer_id.0[..]);
        ad_prefix.extend_from_slice(&container_id.0[..]);
        let key_hasher = Hasher::new_keyed(key.0.expose_secret());
        Self {
            ad_prefix,
            key_hasher,
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
        let (ad, cipher, nonce) = self.associated_data_and_key(entry_idx, slot_idx);
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to encrypt in place
        cipher
            .encrypt_in_place(&nonce, &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res)
    }

    fn associated_data_and_key(
        &self,
        entry_idx: u64,
        slot_idx: u64,
    ) -> (Vec<u8>, Aes256GcmSiv, Nonce) {
        // TODO derive subkey
        let mut ad = Vec::new();
        const AD_LEN: usize = 1 + 32 + 16 + 16 + 8 + 8;
        ad.reserve_exact(AD_LEN);
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&entry_idx.to_le_bytes());
        ad.extend_from_slice(&slot_idx.to_le_bytes());
        let mut key_hasher = self.key_hasher.clone();
        key_hasher.update(&ad);
        let cipher = Aes256GcmSiv::new(key_hasher.finalize().as_bytes().into());
        (ad, cipher, [0; 12].into()) // since we derive every key based on coordinates, we can use a zero nonce
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
        let (ad, cipher, nonce) = self.associated_data_and_key(entry_idx, slot_idx);
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to decrypt in place
        cipher
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

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use test_strategy::proptest;

    use crate::ids::ContainerId;
    use crate::{
        crypto::{CipherSuite, EntryCipher, Key256},
        ids::PeerId,
    };

    #[proptest]
    fn test_roundtrip(
        key: [u8; 32],
        peer_id: [u8; 32],
        container_id: [u8; 16],
        entry_idx: u64,
        op_or_header: Option<u64>,
        slot_bytes: Vec<u8>,
    ) {
        let key = Key256(SecretBox::new(Box::new(key)));
        let cipher = EntryCipher::new(
            CipherSuite::Aes256GcmSiv,
            key,
            &PeerId(peer_id),
            &ContainerId(container_id),
        );
        if let Some(idx) = op_or_header {
            let encrypted = cipher.encrypt_op(entry_idx, idx, &slot_bytes).unwrap();
            let decrypted = cipher.decrypt_op(entry_idx, idx, &encrypted).unwrap();
            assert_eq!(slot_bytes, decrypted);
        } else {
            let encrypted = cipher.encrypt_header(entry_idx, &slot_bytes).unwrap();
            let decrypted = cipher.decrypt_header(entry_idx, &encrypted).unwrap();
            assert_eq!(slot_bytes, decrypted);
        }
    }
}
