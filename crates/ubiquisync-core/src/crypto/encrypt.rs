use chacha20poly1305::AeadInOut;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::inout::InOutBuf;
use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::crypto::Error;
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

impl EntryCipher {
    pub fn new(
        cipher: XChaCha20Poly1305,
        fingerprint: &[u8; 32],
        peer_id: &Uuid,
        container_id: &Uuid,
    ) -> Self {
        let mut ad_prefix = vec![];
        ad_prefix.push(CipherSuite::XChaCha20Poly1305.into());
        ad_prefix.extend_from_slice(&fingerprint[..]);
        ad_prefix.extend_from_slice(&peer_id[..]);
        ad_prefix.extend_from_slice(&container_id[..]);
        Self { ad_prefix, cipher }
    }

    pub fn encrypt_header(&self, entry_idx: u64, header: &[u8]) -> Result<Vec<u8>, Error> {
        self.encrypt_slot(entry_idx, 0, header)
    }

    pub fn encrypt_op(&self, entry_idx: u64, op_index: u64, op: &[u8]) -> Result<Vec<u8>, Error> {
        self.encrypt_slot(entry_idx, op_index + 1, op) // convert to 1-based index, 0 for header
    }

    fn encrypt_slot(&self, entry_idx: u64, slot_idx: u64, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        let mut ad = Vec::new();
        const AD_LEN: usize = 1 + 32 + 16 + 16 + 8 + 8;
        ad.reserve_exact(AD_LEN);
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&entry_idx.to_le_bytes());
        ad.extend_from_slice(&slot_idx.to_le_bytes());
        let nonce: [u8; 24] = blake3::derive_key(DOMAIN_SEPARTOR, &ad[..])[0..24]
            .try_into()
            .unwrap();
        let mut res = vec![0; bytes.len()];
        let inout = InOutBuf::new(bytes, res.as_mut_slice()).map_err(|_| Error::CipherError)?;
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce.into(), &ad, inout)
            .map_err(|_| Error::CipherError)?;
        res.extend_from_slice(tag.as_slice());
        todo!()
    }
}

const DOMAIN_SEPARTOR: &str = "ubiquisync/v1/aead-nonce";
