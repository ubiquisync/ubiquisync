use hpke::{
    Deserializable, Serializable,
    aead::AesGcm256,
    kdf::HkdfSha256,
    kem::{Kem, X25519HkdfSha256},
};
use secrecy::ExposeSecret;
use thiserror::Error;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::Key256,
    log_entry::DecodeError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum EncapsulationKey {
    X25519([u8; 32]),
    P256([u8; 33]),
}

#[derive(Error, Debug)]
#[error("TODO: better error")]
pub struct KeyWrapError;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum KeyWrap {
    X25519HkdfSha256AesGcm256 {
        encapped_key: [u8; 32],
        ciphertext: [u8; 48],
    },
    DhP256HkdfSha256AesGcm256 {
        encapped_key: [u8; 33],
        ciphertext: [u8; 48],
    },
}

pub const DOMAIN_KEY_WRAP: &[u8] = b"ubiquisync/v1/key-wrap";

impl EncapsulationKey {
    pub fn wrap_key(&self, key: &Key256) -> Result<KeyWrap, KeyWrapError> {
        match self {
            EncapsulationKey::X25519(pubkey) => {
                let pubkey = <X25519HkdfSha256 as Kem>::PublicKey::from_bytes(pubkey)
                    .map_err(|_| KeyWrapError)?;
                let (encapped_key, ciphertext) =
                    hpke::single_shot_seal::<AesGcm256, HkdfSha256, X25519HkdfSha256>(
                        &hpke::OpModeS::Base,
                        &pubkey,
                        DOMAIN_KEY_WRAP,
                        key.0.expose_secret(),
                        &[],
                    )
                    .map_err(|_| KeyWrapError)?;
                Ok(KeyWrap::X25519HkdfSha256AesGcm256 {
                    encapped_key: encapped_key
                        .to_bytes()
                        .try_into()
                        .map_err(|_| KeyWrapError)?,
                    ciphertext: ciphertext.try_into().map_err(|_| KeyWrapError)?,
                })
            }
            EncapsulationKey::P256(_) => todo!(),
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

    pub fn decode(reader: &mut Reader) -> Result<Self, DecodeError> {
        Ok(match reader.read_byte()? {
            KEM_ALGO_X25519 => EncapsulationKey::X25519(reader.read_array()?),
            KEM_ALGO_P256 => EncapsulationKey::P256(reader.read_array()?),
            n => return Err(DecodeError::UnknownKeyExchangeAlgorithm(n)),
        })
    }
}

const KEM_ALGO_X25519: u8 = 0;
const KEM_ALGO_P256: u8 = 1;
