use hkdf::Hkdf;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::{ExposeSecret, SecretBox};
use sha2::{Digest, Sha256};

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::{Key256, Key256Fingerprint},
    log_entry::DecodeError,
};

pub type Hash256 = [u8; 32];

// TODO maybe switch to sha256 everywhere
#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Hash256Suite {
    Sha256 = 0,
    Blake3 = 1,
}

#[derive(Clone)]
pub struct Hasher(HasherInternal);

#[derive(Clone)]
enum HasherInternal {
    // TODO feature flags for supported hashes at build time
    Sha256(Sha256),
    Blake3(blake3::Hasher),
}

#[derive(Debug, Clone, Copy)]
pub struct TaggedHashDomain(&'static str);

#[derive(Debug, Clone, Copy)]
pub struct KdfDomain(&'static str);

impl Hash256Suite {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte((*self).into())
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, DecodeError> {
        let x = reader.read_byte()?;
        let suite = Self::try_from_primitive(x).map_err(|_| DecodeError::UnknownHashSuite(x))?;
        Ok(suite)
    }

    pub fn tagged_hash(&self, domain: TaggedHashDomain, data: &[u8]) -> Hash256 {
        let mut hasher = self.tagged_hasher(domain);
        hasher.update(data);
        hasher.finalize()
    }

    pub fn tagged_hasher(&self, domain: TaggedHashDomain) -> Hasher {
        Hasher(match self {
            // TODO feature flags for supported hashes at build time
            Hash256Suite::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(&[domain
                    .0
                    .len()
                    .try_into()
                    .expect("length should statically be < 256")]);
                hasher.update(domain.0);
                HasherInternal::Sha256(hasher)
            }
            Hash256Suite::Blake3 => {
                let hasher = blake3::Hasher::new_derive_key(domain.0);
                HasherInternal::Blake3(hasher)
            }
        })
    }

    pub fn derive_key(&self, domain: KdfDomain, key: &Key256, info: &[u8]) -> Key256 {
        match self {
            Hash256Suite::Sha256 => {
                let hk = Hkdf::<Sha256>::from_prk(key.0.expose_secret()).expect("32 byte key");
                let mut okm = [0; 32];
                let domain_len: u8 = domain.0.len().try_into().expect("max length 255");
                hk.expand_multi_info(&[&[domain_len], domain.0.as_bytes(), info], &mut okm)
                    .expect("valid lengths");
                Key256(SecretBox::new(Box::new(okm)))
            }
            Hash256Suite::Blake3 => {
                let mut hasher = blake3::Hasher::new_keyed(key.0.expose_secret());
                hasher.update(domain.0.as_bytes());
                hasher.update(info);
                Key256(SecretBox::new(Box::new(hasher.finalize().into())))
            }
        }
    }

    pub fn key_fingerprint(&self, domain: KdfDomain, key: &Key256) -> Key256Fingerprint {
        Key256Fingerprint(*self.derive_key(domain, key, &[]).0.expose_secret())
    }
}

impl TaggedHashDomain {
    pub const fn new(domain: &'static str) -> Self {
        assert!(domain.len() < 256, "TaggedHashDomain is too long");
        Self(domain)
    }
}

impl KdfDomain {
    pub const fn new(domain: &'static str) -> Self {
        assert!(domain.len() < 256, "KdfDomain is too long");
        Self(domain)
    }
}

impl Hasher {
    pub fn update(&mut self, data: &[u8]) {
        // TODO feature flags for supported hashes at build time
        match self.0 {
            HasherInternal::Sha256(ref mut hasher) => {
                hasher.update(data);
            }
            HasherInternal::Blake3(ref mut hasher) => {
                hasher.update(data);
            }
        }
    }

    pub fn finalize(self) -> Hash256 {
        match self.0 {
            // TODO feature flags for supported hashes at build time
            HasherInternal::Sha256(hasher) => hasher.finalize().into(),
            HasherInternal::Blake3(hasher) => hasher.finalize().into(),
        }
    }
}
