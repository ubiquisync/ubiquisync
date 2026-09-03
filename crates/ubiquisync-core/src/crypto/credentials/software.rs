use crate::crypto::{
    credentials::Credentials, ed25519::Ed25519SigningKey, kem::X25519DecapsulationKey,
};

pub struct SoftwareCredentials {
    signing_key: Ed25519SigningKey,
    decap_key: X25519DecapsulationKey,
}

impl SoftwareCredentials {
    pub fn new(signing_key: SecretBox<[u8; 32]>, decapsulation_key: SecretBox<[u8; 32]>) -> Self {
        Self {
            signing_key: Ed25519SigningKey::new(signing_key),
            decap_key: X25519DecapsulationKey::new(decapsulation_key),
        }
    }

    pub fn generate() -> Self {
        Self {
            signing_key: Ed25519SigningKey::generate(),
            decap_key: X25519DecapsulationKey::generate(),
        }
    }
}

impl Credentials for SoftwareCredentials {
    fn signing_key(&self) -> &dyn crate::crypto::SigningKey {
        &self.signing_key
    }

    fn decapsulation_key(&self) -> &dyn crate::crypto::kem::DecapsulationKey {
        &self.decap_key
    }
}
