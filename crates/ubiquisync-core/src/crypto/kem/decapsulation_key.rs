use crate::crypto::{
    Key256, Key256Fingerprint,
    kem::{KemError, KeyWrap},
};

pub trait DecapsulationKey {
    fn unwrap_key(&self, expected: &Key256Fingerprint, wrap: &KeyWrap) -> Result<Key256, KemError>;
}
