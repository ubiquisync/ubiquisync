use crate::crypto::kem::EncapsulationKey;

pub trait DecapsulationKey {
    fn encapsulation_key(&self) -> EncapsulationKey;
}
