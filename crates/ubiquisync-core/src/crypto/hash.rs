use hkdf::Hkdf;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::{ExposeSecret, SecretBox};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    codec::{
        reader::{ReadError, Reader},
        writer::Writer,
    },
    crypto::{Key256, Key256Fingerprint},
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

#[derive(Error, Debug)]
pub enum Hash256SuiteDecodeError {
    #[error("unknown hash suite: {0}")]
    UnknownHashSuite(u8),
    #[error("read error: {0}")]
    ReadError(#[from] ReadError),
}

impl Hash256Suite {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte((*self).into())
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, Hash256SuiteDecodeError> {
        let x = reader.read_byte()?;
        let suite = Self::try_from_primitive(x)
            .map_err(|_| Hash256SuiteDecodeError::UnknownHashSuite(x))?;
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
                hasher.update(&[domain.len()]);
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
        Key256(SecretBox::<[u8; 32]>::init_with_mut(|output| {
            self.kdf(domain, key, info, output)
        }))
    }

    pub fn key_fingerprint(&self, domain: KdfDomain, key: &Key256) -> Key256Fingerprint {
        let mut output = [0; 32];
        self.kdf(domain, key, &[], &mut output);
        Key256Fingerprint(output)
    }

    fn kdf(&self, domain: KdfDomain, key: &Key256, info: &[u8], output: &mut [u8; 32]) {
        let domain_len = domain.len();
        match self {
            Hash256Suite::Sha256 => {
                let hk = Hkdf::<Sha256>::from_prk(key.0.expose_secret()).expect("32 byte key");
                hk.expand_multi_info(&[&[domain_len], domain.0.as_bytes(), info], output)
                    .expect("valid lengths");
            }
            Hash256Suite::Blake3 => {
                let mut hasher = blake3::Hasher::new_keyed(key.0.expose_secret());
                hasher.update(&[domain_len]);
                hasher.update(domain.0.as_bytes());
                hasher.update(info);
                hasher.finalize_xof().fill(output);
            }
        }
    }
}

impl TaggedHashDomain {
    pub const fn new(domain: &'static str) -> Self {
        assert!(domain.len() < 256, "TaggedHashDomain is too long");
        Self(domain)
    }

    pub fn len(&self) -> u8 {
        self.0.len().try_into().expect("max length 255")
    }
}

impl KdfDomain {
    pub const fn new(domain: &'static str) -> Self {
        assert!(domain.len() < 256, "KdfDomain is too long");
        Self(domain)
    }

    pub fn len(&self) -> u8 {
        self.0.len().try_into().expect("max length 255")
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

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;

    use crate::crypto::{Hash256Suite, KdfDomain, Key256, TaggedHashDomain};

    #[test]
    fn test_kdf_len_prefixed() {
        do_test_kdf_len_prefixed(Hash256Suite::Sha256);
        do_test_kdf_len_prefixed(Hash256Suite::Blake3);
    }

    fn do_test_kdf_len_prefixed(suite: Hash256Suite) {
        let key = Key256(SecretBox::new(Box::new([1; 32])));
        let mut x = [0; 32];
        let mut y = [0; 32];
        suite.kdf(KdfDomain::new(&"a"), &key, b"bc", &mut x);
        suite.kdf(KdfDomain::new(&"ab"), &key, b"c", &mut y);
        assert_ne!(x, [0; 32]);
        assert_ne!(y, [0; 32]);
        assert_ne!(x, y);
    }

    #[test]
    fn test_tagged_hash_len_prefixed() {
        do_test_tagged_hash_len_prefixed(Hash256Suite::Sha256);
        do_test_tagged_hash_len_prefixed(Hash256Suite::Blake3);
    }

    fn do_test_tagged_hash_len_prefixed(suite: Hash256Suite) {
        let x = suite.tagged_hash(TaggedHashDomain::new(&"a"), b"bc");
        let y = suite.tagged_hash(TaggedHashDomain::new(&"ab"), b"c");
        assert_ne!(x, y)
    }
}
