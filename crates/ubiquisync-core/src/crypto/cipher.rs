use std::borrow::Borrow;
use std::ops::Range;

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

use crate::bytes::OpaqueBytes;
use crate::bytes::PlaintextBytes;
use crate::codec::ReadError;
use crate::codec::Reader;
use crate::codec::Writer;
use crate::crypto::DeriveKeyDomain;
use crate::crypto::Hash256;
use crate::crypto::Hash256Suite;
use crate::ids::LogId;

#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum CipherSuite {
    Aes256GcmSiv = 0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct CipherInfo {
    pub cipher_suite: u8,
    pub fingerprint: Key256Fingerprint,
}

pub struct EntryCipher {
    fingerprint: Key256Fingerprint,
    key: Key256,
    ad_prefix: Vec<u8>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Key256(pub SecretBox<[u8; 32]>);

impl Key256 {
    pub fn fingerprint(&self) -> Key256Fingerprint {
        Key256Fingerprint(blake3::derive_key(
            DOMAIN_KEY_FINGERPRINT,
            self.0.expose_secret(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct Key256Fingerprint(pub [u8; 32]);

const DOMAIN_KEY_FINGERPRINT: &str = "ubiquisync/v1/key-fingerprint";

#[derive(Error, Debug)]
#[error("cipher error")]
pub struct CipherError;

impl EntryCipher {
    pub fn new(suite: CipherSuite, key: Key256, log_id: &LogId) -> Self {
        assert_eq!(
            suite,
            CipherSuite::Aes256GcmSiv,
            "if this gets triggered it means we need to support new cipher suites",
        );

        let mut ad_prefix = vec![];
        ad_prefix.push(suite.into());
        let fingerprint = key.fingerprint();
        ad_prefix.extend_from_slice(&fingerprint.0[..]);
        ad_prefix.extend_from_slice(&log_id.peer_id.0[..]);
        ad_prefix.extend_from_slice(&log_id.container_id.0[..]);
        Self {
            ad_prefix,
            key,
            fingerprint,
        }
    }

    pub fn encrypt_header(
        &self,
        entry_idx: u64,
        prev_hash: &Hash256,
        header: &PlaintextBytes,
    ) -> Result<OpaqueBytes<'static>, CipherError> {
        self.encrypt_slot(entry_idx, 0, prev_hash, header)
    }

    pub fn encrypt_op(
        &self,
        entry_idx: u64,
        op_index: u64,
        prev_hash: &Hash256,
        op: &PlaintextBytes,
    ) -> Result<OpaqueBytes<'static>, CipherError> {
        self.encrypt_slot(
            entry_idx,
            op_index.checked_add(1).expect("op index overflow"),
            prev_hash,
            op,
        ) // convert to 1-based index, 0 for header
    }

    fn encrypt_slot(
        &self,
        entry_idx: u64,
        slot_idx: u64,
        prev_hash: &Hash256,
        bytes: &PlaintextBytes,
    ) -> Result<OpaqueBytes<'static>, CipherError> {
        let (ad, cipher, nonce) = self.associated_data_and_key(entry_idx, slot_idx, prev_hash);
        let bytes: &[u8] = bytes.borrow();
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to encrypt in place
        cipher
            .encrypt_in_place(&nonce, &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res.into())
    }

    fn associated_data_and_key(
        &self,
        entry_idx: u64,
        slot_idx: u64,
        prev_hash: &Hash256,
    ) -> (Vec<u8>, Aes256GcmSiv, Nonce) {
        let mut ad = Vec::new();
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&entry_idx.to_le_bytes());
        ad.extend_from_slice(&slot_idx.to_le_bytes());
        let mut key_info = ad.clone();
        key_info.extend_from_slice(&prev_hash[..]);
        let key =
            Hash256Suite::Sha256.derive_key(DeriveKeyDomain::EntryCipher, &self.key, &key_info);
        let cipher = Aes256GcmSiv::new(key.0.expose_secret().into());
        (ad, cipher, [0; 12].into()) // since we derive every key based on coordinates, we can use a zero nonce
    }

    pub fn decrypt_header(
        &self,
        entry_idx: u64,
        prev_hash: &Hash256,
        header: &OpaqueBytes,
    ) -> Result<PlaintextBytes<'static>, CipherError> {
        self.decrypt_slot(entry_idx, 0, prev_hash, header)
    }

    pub fn decrypt_op(
        &self,
        entry_idx: u64,
        op_index: u64,
        prev_hash: &Hash256,
        op: &OpaqueBytes,
    ) -> Result<PlaintextBytes<'static>, CipherError> {
        self.decrypt_slot(
            entry_idx,
            op_index.checked_add(1).expect("op index overflow"),
            prev_hash,
            op,
        ) // convert to 1-based index, 0 for header
    }

    fn decrypt_slot(
        &self,
        entry_idx: u64,
        slot_idx: u64,
        prev_hash: &Hash256,
        bytes: &OpaqueBytes,
    ) -> Result<PlaintextBytes<'static>, CipherError> {
        let (ad, cipher, nonce) = self.associated_data_and_key(entry_idx, slot_idx, prev_hash);
        let bytes: &[u8] = bytes.borrow();
        let mut res = Vec::from(bytes); // we copy the input data to the res vec to decrypt in place
        cipher
            .decrypt_in_place(&nonce, &ad, &mut res)
            .map_err(|_| CipherError)?;
        Ok(res.into())
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
    ) -> (Vec<u8>, Aes256GcmSiv, Nonce) {
        let mut ad = Vec::new();
        ad.extend_from_slice(self.ad_prefix.as_slice());
        ad.extend_from_slice(&range.start.to_le_bytes());
        ad.extend_from_slice(&range.end.to_le_bytes());
        // TODO do we need end in AD too?
        let mut key_info = ad.clone();
        key_info.extend_from_slice(nonce);
        let key =
            Hash256Suite::Sha256.derive_key(DeriveKeyDomain::EntryCipher, &self.key, &key_info);
        let cipher = Aes256GcmSiv::new(key.0.expose_secret().into());
        (ad, cipher, [0; 12].into()) // since we derive every key based on coordinates, we can use a zero nonce
    }

    pub fn key_fingerprint(&self) -> &Key256Fingerprint {
        &self.fingerprint
    }

    pub fn cipher_suite(&self) -> CipherSuite {
        CipherSuite::Aes256GcmSiv
    }

    pub fn cipher_info(&self) -> CipherInfo {
        CipherInfo {
            cipher_suite: self.cipher_suite().into(),
            fingerprint: self.fingerprint,
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
            fingerprint: Key256Fingerprint(reader.read_array()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::bytes::PlaintextBytes;
    use crate::ids::{ContainerId, LogId};
    use crate::{
        crypto::{CipherSuite, EntryCipher, Key256},
        ids::PeerId,
    };

    #[proptest]
    fn test_roundtrip(
        key: [u8; 32],
        peer_id: [u8; 32],
        container_id: [u8; 16],
        last_hash: [u8; 32],
        entry_idx: u64,
        op_or_header: Option<u64>,
        slot_bytes: Vec<u8>,
    ) {
        let key = Key256(SecretBox::new(Box::new(key)));
        let log_id = LogId {
            peer_id: PeerId(peer_id),
            container_id: ContainerId(container_id),
        };
        let cipher = EntryCipher::new(CipherSuite::Aes256GcmSiv, key, &log_id);
        let slot_bytes = PlaintextBytes(slot_bytes.into());
        if let Some(idx) = op_or_header {
            let encrypted = cipher
                .encrypt_op(entry_idx, idx, &last_hash, &slot_bytes)
                .unwrap();
            let decrypted = cipher
                .decrypt_op(entry_idx, idx, &last_hash, &encrypted)
                .unwrap();
            assert_eq!(slot_bytes, decrypted);
        } else {
            let encrypted = cipher
                .encrypt_header(entry_idx, &last_hash, &slot_bytes)
                .unwrap();
            let decrypted = cipher
                .decrypt_header(entry_idx, &last_hash, &encrypted)
                .unwrap();
            assert_eq!(slot_bytes, decrypted);
        }
    }

    // TODO snapshot tests
}
