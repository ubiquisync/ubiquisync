use secrecy::{ExposeSecret, SecretBox};

use crate::crypto::{Signature, SigningError, SigningKey, VerifyingKey};

pub struct Ed25519SigningKey {
    sk: SecretBox<[u8; 32]>,
    pk: VerifyingKey,
}

impl Ed25519SigningKey {
    pub fn new(sk: SecretBox<[u8; 32]>) -> Self {
        let ss = ed25519_dalek::SigningKey::from_bytes(sk.expose_secret());
        let pk = VerifyingKey::Ed25519(ss.verifying_key().to_bytes());
        Self { sk, pk }
    }

    pub fn generate() -> Self {
        let key = SecretBox::<[u8; 32]>::init_with_mut(|buf| {
            getrandom::fill(buf).expect("random number")
        });
        Self::new(key)
    }
}

impl SigningKey for Ed25519SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError> {
        use ed25519_dalek::Signer;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(self.sk.expose_secret());
        let sig = signing_key.try_sign(message).map_err(|_| SigningError)?;
        Ok(Signature::Ed25519(sig.to_bytes()))
    }

    fn verifying_key(&self) -> VerifyingKey {
        self.pk
    }
}
