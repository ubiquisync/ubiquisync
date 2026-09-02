use crate::crypto::kem::EncapsulationKey;

pub trait DecapsulationKey {
    fn encapsulation_key(&self) -> EncapsulationKey;
    // TODO
    // fn unwrap_key(&self, expected: &Key256Fingerprint, wrap: &KeyWrap) -> Result<Key256, KemError>;
}
