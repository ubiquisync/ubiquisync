use hkdf::Hkdf;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::{ExposeSecret, SecretBox};
use sha2::{Digest, Sha256};
use strum_macros::{EnumIter, IntoStaticStr};
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

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubiquisync/v1/hash/")]
pub enum TaggedHashDomain {
    MmrNode,
    MmrSeed,
    MmrBag,
    MmrSignBytes,
}

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubiquisync/v1/kdf/")]
pub enum DeriveKeyDomain {
    EncryptionKey,
}

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubiquisync/v1/fingerprint/")]
pub enum KeyFingerprintDomain {
    EncryptionKey,
}

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
        self.tagged_hash_internal(domain.into(), data)
    }

    fn tagged_hash_internal(&self, domain: &str, data: &[u8]) -> Hash256 {
        let mut hasher = self.new_tagged_hasher_internal(domain);
        hasher.update(data);
        hasher.finalize()
    }

    pub fn new_tagged_hasher(&self, domain: TaggedHashDomain) -> Hasher {
        self.new_tagged_hasher_internal(domain.into())
    }

    fn new_tagged_hasher_internal(&self, domain: &str) -> Hasher {
        let len: u8 = domain_len(domain);
        Hasher(match self {
            // TODO feature flags for supported hashes at build time
            Hash256Suite::Sha256 => {
                let mut hasher = Sha256::new();
                hasher.update(&[len]);
                hasher.update(domain);
                HasherInternal::Sha256(hasher)
            }
            Hash256Suite::Blake3 => {
                let hasher = blake3::Hasher::new_derive_key(domain);
                HasherInternal::Blake3(hasher)
            }
        })
    }

    pub fn derive_key(&self, domain: DeriveKeyDomain, key: &Key256, info: &[u8]) -> Key256 {
        Key256(SecretBox::<[u8; 32]>::init_with_mut(|output| {
            self.kdf(domain.into(), key, info, output)
        }))
    }

    pub fn key_fingerprint(&self, domain: KeyFingerprintDomain, key: &Key256) -> Key256Fingerprint {
        let mut output = [0; 32];
        self.kdf(domain.into(), key, &[], &mut output);
        Key256Fingerprint(output)
    }

    fn kdf(&self, domain: &str, key: &Key256, info: &[u8], output: &mut [u8; 32]) {
        let len: u8 = domain_len(domain);
        match self {
            Hash256Suite::Sha256 => {
                let hk = Hkdf::<Sha256>::from_prk(key.0.expose_secret()).expect("32 byte key");
                hk.expand_multi_info(&[&[len], domain.as_bytes(), info], output)
                    .expect("valid lengths");
            }
            Hash256Suite::Blake3 => {
                let mut hasher = blake3::Hasher::new_keyed(key.0.expose_secret());
                hasher.update(&[len]);
                hasher.update(domain.as_bytes());
                hasher.update(info);
                hasher.finalize_xof().fill(output);
            }
        }
    }
}

fn domain_len(domain: &str) -> u8 {
    domain
        .len()
        .try_into()
        .expect("domain string should have len <= 255")
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
    use insta::assert_snapshot;
    use secrecy::{ExposeSecret, SecretBox};
    use std::fmt::Write;
    use strum::IntoEnumIterator;

    use crate::crypto::{
        DeriveKeyDomain, Hash256Suite, Key256, KeyFingerprintDomain, TaggedHashDomain,
    };

    #[test]
    fn test_kdf_len_prefixed() {
        do_test_kdf_len_prefixed(Hash256Suite::Sha256);
        do_test_kdf_len_prefixed(Hash256Suite::Blake3);
    }

    fn do_test_kdf_len_prefixed(suite: Hash256Suite) {
        let key = Key256(SecretBox::new(Box::new([1; 32])));
        let mut x = [0; 32];
        let mut y = [0; 32];
        suite.kdf("a", &key, b"bc", &mut x);
        suite.kdf("ab", &key, b"c", &mut y);
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
        let x = suite.tagged_hash_internal("a", b"bc");
        let y = suite.tagged_hash_internal("ab", b"c");
        assert_ne!(x, y)
    }

    #[test]
    fn test_sha256_domain_regression() {
        assert_snapshot!(test_domain_regression(Hash256Suite::Sha256));
    }

    #[test]
    fn test_blake3_domain_regression() {
        assert_snapshot!(test_domain_regression(Hash256Suite::Blake3));
    }

    // prevents hash regressions from renaming items in the domain enum
    fn test_domain_regression(suite: Hash256Suite) -> String {
        let mut out = format!("{suite:?}\n");
        let key = Key256(SecretBox::new(Box::new([1; 32])));

        for domain in TaggedHashDomain::iter() {
            let h = suite.tagged_hash(domain, b"abcdefg");
            let d: &str = domain.into();
            writeln!(&mut out, "{0} = {1}", d, hex(&h)).unwrap();

            // check that both hash methods produce the same output
            let mut hasher = suite.new_tagged_hasher(domain);
            hasher.update(b"abc");
            hasher.update(b"defg");
            let h2 = hasher.finalize();
            assert_eq!(h, h2);
        }

        for domain in DeriveKeyDomain::iter() {
            let d: &str = domain.into();
            let k = suite.derive_key(domain, &key, b"");
            writeln!(&mut out, "{0} = {1}", d, hex(k.0.expose_secret())).unwrap();

            let k2 = suite.derive_key(domain, &key, b"xyz");
            writeln!(&mut out, "{0} / xyz = {1}", d, hex(k2.0.expose_secret())).unwrap();
        }

        for domain in KeyFingerprintDomain::iter() {
            let f = suite.key_fingerprint(domain, &key);
            let d: &str = domain.into();
            writeln!(&mut out, "{0} = {1}", d, hex(&f.0)).unwrap();
        }
        out
    }

    #[test]
    fn test_domain_lengths() {
        for domain in TaggedHashDomain::iter() {
            let s: &str = domain.into();
            assert!(s.len() <= 255);
        }

        for domain in DeriveKeyDomain::iter() {
            let s: &str = domain.into();
            assert!(s.len() <= 255);
        }

        for domain in KeyFingerprintDomain::iter() {
            let s: &str = domain.into();
            assert!(s.len() <= 255);
        }
    }

    fn hex(hash: &[u8; 32]) -> String {
        let mut s = String::new();
        for b in hash {
            write!(s, "{b:02x}").unwrap()
        }
        s
    }
}
