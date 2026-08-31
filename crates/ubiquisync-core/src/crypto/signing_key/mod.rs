#[cfg(target_vendor = "apple")]
pub mod apple;

use thiserror::Error;

use crate::crypto::{Signature, VerifyingKey};

pub trait SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError>;
    // NOTE: named get_verifying_key just to not conflict with verifying_key which many keys define
    fn get_verifying_key(&self) -> VerifyingKey;
}

#[derive(Error, Debug)]
#[error("sign error")]
pub struct SigningError;

impl SigningKey for ed25519_dalek::SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError> {
        use ed25519_dalek::Signer;

        let sig = self.try_sign(&message).map_err(|_| SigningError)?;
        Ok(Signature::Ed25519(sig.to_bytes()))
    }

    fn get_verifying_key(&self) -> VerifyingKey {
        VerifyingKey::Ed25519(self.verifying_key().to_bytes())
    }
}

// For now only test config because we only want to support ed25519 for software signing.
// Hardware signers usually must use P256, software signers can use ed25519.
#[cfg(test)]
impl SigningKey for p256::ecdsa::SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError> {
        use ed25519_dalek::Signer;
        let sig: p256::ecdsa::Signature = self.try_sign(&message).map_err(|_| SigningError)?;
        Ok(Signature::P256(sig.normalize_s().to_bytes().into()))
    }

    fn get_verifying_key(&self) -> VerifyingKey {
        VerifyingKey::P256(
            self.verifying_key()
                .to_sec1_point(true)
                .as_bytes()
                .try_into()
                .expect("expected 33 bytes"),
        )
    }
}
