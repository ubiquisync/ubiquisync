use secrecy::{ExposeSecret, SecretBox};

use crate::crypto::kem::{DecapsulationKey, EncapsulationKey};

pub struct X25519DecapsulationKey {
    #[allow(dead_code)]
    sk: SecretBox<[u8; 32]>,
    pk: EncapsulationKey,
}

impl X25519DecapsulationKey {
    pub fn new(sk: SecretBox<[u8; 32]>) -> Self {
        let ss = x25519_dalek::StaticSecret::from(*sk.expose_secret());
        let pk = x25519_dalek::PublicKey::from(&ss);
        Self {
            sk,
            pk: EncapsulationKey::X25519(pk.to_bytes()),
        }
    }

    pub fn generate() -> Self {
        let key = SecretBox::<[u8; 32]>::init_with_mut(|buf| {
            getrandom::fill(buf).expect("random number")
        });
        Self::new(key)
    }
}

impl DecapsulationKey for X25519DecapsulationKey {
    fn encapsulation_key(&self) -> EncapsulationKey {
        self.pk
    }
}
