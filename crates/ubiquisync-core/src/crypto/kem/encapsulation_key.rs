use hpke::{
    Deserializable, Serializable,
    aead::ChaCha20Poly1305,
    kdf::HkdfSha256,
    kem::{Kem, X25519HkdfSha256},
};
use secrecy::{ExposeSecret, SecretBox};
use thiserror::Error;

use crate::{
    codec::{Reader, Writer},
    crypto::CryptoDecodeError,
    ids::ContainerId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EncapsulationKey {
    X25519([u8; 32]),
    P256([u8; 33]),
}

#[derive(Error, Debug)]
#[error("KEM error")]
pub struct KemError;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum KeyWrap {
    X25519HkdfSha256ChaCha20Poly1305 { enc: [u8; 32], ciphertext: [u8; 48] },
    DhP256HkdfSha256ChaCha20Poly1305 { enc: [u8; 33], ciphertext: [u8; 48] },
}

pub struct ScopedKeyWrap {
    pub wrap: KeyWrap,
    pub scope: KeyScope,
}

pub enum KeyScope {
    Root,
    Container(ContainerId),
}

pub const DOMAIN_KEY_WRAP: &[u8] = b"ubiquisync/v1/key-wrap";

impl EncapsulationKey {
    #[allow(dead_code)]
    fn do_wrap_key(&self, key: &SecretBox<[u8; 32]>) -> Result<KeyWrap, KemError> {
        match self {
            EncapsulationKey::X25519(pubkey) => {
                let pubkey = <X25519HkdfSha256 as Kem>::PublicKey::from_bytes(pubkey)
                    .map_err(|_| KemError)?;
                let (encapped_key, ciphertext) =
                    hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, X25519HkdfSha256>(
                        &hpke::OpModeS::Base,
                        &pubkey,
                        DOMAIN_KEY_WRAP,
                        key.expose_secret(),
                        &[],
                    )
                    .map_err(|_| KemError)?;
                Ok(KeyWrap::X25519HkdfSha256ChaCha20Poly1305 {
                    enc: encapped_key.to_bytes().into(),
                    ciphertext: ciphertext.try_into().map_err(|_| KemError)?,
                })
            }
            EncapsulationKey::P256(_) => todo!("implement p256 key encapsulation"),
        }
    }

    pub fn encode(&self, writer: &mut Writer) {
        match self {
            EncapsulationKey::X25519(key) => {
                writer.write_byte(KEM_ALGO_X25519);
                writer.write_array(key);
            }
            EncapsulationKey::P256(key) => {
                writer.write_byte(KEM_ALGO_P256);
                writer.write_array(key);
            }
        }
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, CryptoDecodeError> {
        Ok(match reader.read_byte()? {
            KEM_ALGO_X25519 => EncapsulationKey::X25519(reader.read_array()?),
            KEM_ALGO_P256 => EncapsulationKey::P256(reader.read_array()?),
            n => return Err(CryptoDecodeError::UnknownAlgorithm(n)),
        })
    }
}

const KEM_ALGO_X25519: u8 = 0;
const KEM_ALGO_P256: u8 = 1;
