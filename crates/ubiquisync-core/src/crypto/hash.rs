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
#[strum(prefix = "ubiquisync/v1/")]
pub enum TaggedHashDomain {
    MmrNode,
    MmrSeed,
    MmrBag,
    MmrSignBytes,
}

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
        self.tagged_hash_internal(domain.into(), data)
    }

    fn tagged_hash_internal(&self, domain: &str, data: &[u8]) -> Hash256 {
        let mut hasher = self.tagged_hasher_internal(domain);
        hasher.update(data);
        hasher.finalize()
    }

    pub fn tagged_hasher(&self, domain: TaggedHashDomain) -> Hasher {
        self.tagged_hasher_internal(domain.into())
    }

    fn tagged_hasher_internal(&self, domain: &str) -> Hasher {
        let len: u8 = domain
            .len()
            .try_into()
            .expect("domain string should have len <= 255");
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

impl KdfDomain {
    pub const fn new(domain: &'static str) -> Self {
        assert!(domain.len() < 256, "KdfDomain is too long");
        Self(domain)
    }

    fn len(&self) -> u8 {
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
    use insta::assert_snapshot;
    use secrecy::{ExposeSecret, SecretBox};
    use std::fmt::Write;
    use strum::IntoEnumIterator;

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
        suite.kdf(KdfDomain::new("a"), &key, b"bc", &mut x);
        suite.kdf(KdfDomain::new("ab"), &key, b"c", &mut y);
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
    fn test_known_sha256() {
        test_known_hashes(
            Hash256Suite::Sha256,
            &[
                "1e18834c426d00e57788444cb3ccd62c771b420c095bb0c4e040a8c122c4570d",
                "3d723448c579b5e3e5d771bc0e5deda551db6cc198b0bc975bd30ac6584b62d8",
                "158c46d846b5017928100e816969369dd5fdea2b379fda352922b97ef85045c5",
            ],
        );
    }

    #[test]
    fn test_known_blake3() {
        test_known_hashes(
            Hash256Suite::Blake3,
            &[
                "d12db47e19b120ba45062e4ff453d3d40d1d7c2b8a0ca07c9fe4945d283aee13",
                "4bf31948ffa28caf02dc226346de69d7f2fecf36f3b86e1c3d4cdd508c21a3a7",
                "aab5ab04ca17deb69363735d6c021919d7bca1fcccac66850728af543fa95233",
            ],
        );
    }

    fn test_known_hashes(suite: Hash256Suite, known: &[&str; 3]) {
        let a = suite.tagged_hash_internal("a", b"bc");
        assert_eq!(hex(&a), known[0]);

        let mut h = suite.tagged_hasher_internal("a");
        h.update(b"b");
        h.update(b"c");
        let b = h.finalize();
        assert_eq!(hex(&b), known[0]);

        let key = Key256(SecretBox::new(Box::new([1; 32])));
        let c = suite.key_fingerprint(KdfDomain::new("b"), &key);
        assert_eq!(hex(&c.0), known[1]);

        let d = suite.derive_key(KdfDomain::new("c"), &key, b"xyz");
        assert_eq!(hex(&d.0.expose_secret()), known[2]);
    }

    fn hex(hash: &[u8; 32]) -> String {
        let mut s = String::new();
        for b in hash {
            write!(s, "{b:02x}").unwrap()
        }
        s
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
        let mut out = String::from(format!("{suite:?}\n"));
        for domain in TaggedHashDomain::iter() {
            let h = suite.tagged_hash(domain, b"abcdefg");
            let d: &str = domain.into();
            writeln!(&mut out, "{0} = {1}", d, hex(&h)).unwrap();
        }
        out
    }
}
