use hkdf::Hkdf;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use secrecy::{ExposeSecret, SecretBox};
use sha2::{Digest, Sha256};
use strum_macros::{EnumIter, IntoStaticStr};

use crate::{
    codec::{Reader, Writer},
    crypto::{CryptoDecodeError, Key256, Key256Fingerprint},
};

pub type Hash256 = [u8; 32];

// TODO maybe switch to sha256 everywhere
#[repr(u8)]
#[derive(IntoPrimitive, TryFromPrimitive, Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Hash256Suite {
    Sha256 = 0,
}

#[derive(Clone)]
pub struct Hasher(Sha256);

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubq/v1/hash/")]
pub enum TaggedHashDomain {
    ChainSeed,
    ChainHash,
    LogSignBytes,
    PeerInitCommitment,
    LogEntryOpBatch,
    LogEntryUseKey,
    OpBatchSlot,
    LogEntryNew,
}

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubq/v1/kdf/")]
pub enum DeriveKeyDomain {
    /// Used for per-entry opaque encryption which is canonical for hashing.
    EntryCipher,
    /// Used when encoding a segment (multiple entries) using a nonce in the segment header.
    SegmentCipher,
}

#[derive(IntoStaticStr, EnumIter, Debug, Clone, Copy, PartialEq, Eq)]
#[strum(prefix = "ubq/v1/fingerprint/")]
pub enum KeyFingerprintDomain {
    EncryptionKey,
}

impl Hash256Suite {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_byte((*self).into())
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, CryptoDecodeError> {
        let x = reader.read_byte()?;
        let suite =
            Self::try_from_primitive(x).map_err(|_| CryptoDecodeError::UnknownAlgorithm(x))?;
        Ok(suite)
    }
}

pub fn tagged_hash(domain: TaggedHashDomain, data: &[u8]) -> Hash256 {
    tagged_hash_internal(domain.into(), data)
}

fn tagged_hash_internal(domain: &str, data: &[u8]) -> Hash256 {
    let mut hasher = new_tagged_hasher_internal(domain);
    hasher.update(data);
    hasher.finalize()
}

pub fn new_tagged_hasher(domain: TaggedHashDomain) -> Hasher {
    new_tagged_hasher_internal(domain.into())
}

fn new_tagged_hasher_internal(domain: &str) -> Hasher {
    let len: u8 = domain_len(domain);
    let mut hasher = Sha256::new();
    hasher.update([len]);
    hasher.update(domain);
    Hasher(hasher)
}

pub fn derive_key(domain: DeriveKeyDomain, key: &Key256, info: &[u8]) -> Key256 {
    Key256(SecretBox::<[u8; 32]>::init_with_mut(|output| {
        kdf(domain.into(), key, info, output)
    }))
}

pub fn key_fingerprint(domain: KeyFingerprintDomain, key: &Key256) -> Key256Fingerprint {
    let mut output = [0; 32];
    kdf(domain.into(), key, &[], &mut output);
    Key256Fingerprint(output)
}

fn kdf(domain: &str, key: &Key256, info: &[u8], output: &mut [u8; 32]) {
    let len: u8 = domain_len(domain);
    let hk = Hkdf::<Sha256>::from_prk(key.0.expose_secret()).expect("32 byte key");
    hk.expand_multi_info(&[&[len], domain.as_bytes(), info], output)
        .expect("valid lengths");
}

fn domain_len(domain: &str) -> u8 {
    domain
        .len()
        .try_into()
        .expect("domain string should have len <= 255")
}

impl Hasher {
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data)
    }

    pub fn finalize(self) -> Hash256 {
        self.0.finalize().into()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use insta::assert_snapshot;
    use secrecy::{ExposeSecret, SecretBox};
    use std::fmt::Write;
    use strum::IntoEnumIterator;

    use crate::crypto::{
        DeriveKeyDomain, Hash256Suite, Key256, KeyFingerprintDomain, TaggedHashDomain, derive_key,
        hash::{kdf, tagged_hash_internal},
        key_fingerprint, new_tagged_hasher, tagged_hash,
    };

    #[test]
    fn test_kdf_len_prefixed() {
        let key = Key256(SecretBox::new(Box::new([1; 32])));
        let mut x = [0; 32];
        let mut y = [0; 32];
        kdf("a", &key, b"bc", &mut x);
        kdf("ab", &key, b"c", &mut y);
        assert_ne!(x, [0; 32]);
        assert_ne!(y, [0; 32]);
        assert_ne!(x, y);
    }

    #[test]
    fn test_tagged_hash_len_prefixed() {
        let x = tagged_hash_internal("a", b"bc");
        let y = tagged_hash_internal("ab", b"c");
        assert_ne!(x, y)
    }

    #[test]
    fn test_sha256_domain_regression() {
        let suite = Hash256Suite::Sha256;
        let mut out = format!("{suite:?}\n");
        let key = Key256(SecretBox::new(Box::new([1; 32])));

        for domain in TaggedHashDomain::iter() {
            let h = tagged_hash(domain, b"abcdefg");
            let d: &str = domain.into();
            writeln!(&mut out, "{0} = {1}", d, hex(&h)).unwrap();

            // check that both hash methods produce the same output
            let mut hasher = new_tagged_hasher(domain);
            hasher.update(b"abc");
            hasher.update(b"defg");
            let h2 = hasher.finalize();
            assert_eq!(h, h2);
        }

        for domain in DeriveKeyDomain::iter() {
            let d: &str = domain.into();
            let k = derive_key(domain, &key, b"");
            writeln!(&mut out, "{0} = {1}", d, hex(k.0.expose_secret())).unwrap();

            let k2 = derive_key(domain, &key, b"xyz");
            writeln!(&mut out, "{0} / xyz = {1}", d, hex(k2.0.expose_secret())).unwrap();
        }

        for domain in KeyFingerprintDomain::iter() {
            let f = key_fingerprint(domain, &key);
            let d: &str = domain.into();
            writeln!(&mut out, "{0} = {1}", d, hex(&f.0)).unwrap();
        }
        assert_snapshot!(out)
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

    pub(crate) fn hex(hash: &[u8; 32]) -> String {
        let mut s = String::new();
        for b in hash {
            write!(s, "{b:02x}").unwrap()
        }
        s
    }
}
