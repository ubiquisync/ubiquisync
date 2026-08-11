use chacha20poly1305::AeadInOut;
use chacha20poly1305::KeyInit;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::XNonce;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::ExposeSecret;
use secrecy::SecretSlice;
use thiserror::Error;
use zeroize::Zeroize;
use zeroize::ZeroizeOnDrop;

use crate::crypto::Hash;
use crate::uuid::Uuid;

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive)]
pub enum CipherSuite {
    XChaCha20Poly1305 = 0,
}

pub struct EntryCipher {
    cipher: XChaCha20Poly1305,
    ad_prefix: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct XChaCha20Poly1305Key(SecretSlice<u8>);

impl XChaCha20Poly1305Key {
    pub fn fingerprint(&self) -> [u8; 32] {
        blake3::derive_key(DOMAIN_KEY_FINGERPRINT, self.0.expose_secret())
    }

    pub fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&self.0.expose_secret().try_into().unwrap()) // TODO don't unwrap
    }
}

const DOMAIN_KEY_FINGERPRINT: &str = "ubiquisync/v1/key-fingerprint";
const DOMAIN_AEAD_NONCE: &str = "ubiquisync/v1/aead-nonce";

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl EntryCipher {
    pub fn new(key: XChaCha20Poly1305Key, peer_id: &Uuid, container_id: &Uuid) -> Self {
        let mut ad_prefix = vec![];
        ad_prefix.push(CipherSuite::XChaCha20Poly1305.into());
        ad_prefix.extend_from_slice(&key.fingerprint()[..]);
        ad_prefix.extend_from_slice(&peer_id[..]);
        ad_prefix.extend_from_slice(&container_id[..]);
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
            .encrypt_in_place(&nonce.into(), &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res)
    }

    fn associated_data_and_nonce(&self, entry_idx: u64, slot_idx: u64) -> (Vec<u8>, XNonce) {
        let mut ad = Vec::new();
        const AD_LEN: usize = 1 + 32 + 16 + 16 + 8 + 8;
        ad.reserve_exact(AD_LEN);
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&entry_idx.to_le_bytes());
        ad.extend_from_slice(&slot_idx.to_le_bytes());
        let nonce: [u8; 24] = blake3::derive_key(DOMAIN_AEAD_NONCE, &ad[..])[0..24]
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
            .decrypt_in_place(&nonce.into(), &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res)
    }
}

pub struct EncryptionKeyRing {}

impl EncryptionKeyRing {
    pub fn get_key(&self, fingerprint: &Hash) -> Option<XChaCha20Poly1305Key> {
        todo!()
    }
}
