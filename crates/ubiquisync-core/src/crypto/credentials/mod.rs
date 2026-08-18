use crate::crypto::{SigningKey, kem::DecapsulationKey};

pub trait Credentials {
    fn signing_key(&self) -> &dyn SigningKey;
    fn decapsulation_key(&self) -> &dyn DecapsulationKey;
}
