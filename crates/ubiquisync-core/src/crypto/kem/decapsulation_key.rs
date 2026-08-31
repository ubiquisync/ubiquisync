use crate::crypto::{KemError, Key256, Key256Fingerprint, KeyWrap};

pub trait DecapsulationKey {
    fn unwrap_key(&self, expected: &Key256Fingerprint, wrap: &KeyWrap) -> Result<Key256, KemError>;
}
