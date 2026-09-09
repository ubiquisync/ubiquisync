use crate::crypto::{SigningKey, kem::DecapsulationKey};

pub mod software;

pub trait Credentials: Send + Sync {
    fn signing_key(&self) -> &dyn SigningKey;
    fn decapsulation_key(&self) -> &dyn DecapsulationKey;
}
