#[cfg(target_vendor = "apple")]
pub mod apple;
pub mod ed25519;

use thiserror::Error;

use crate::crypto::{Signature, VerifyingKey};

pub trait SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError>;
    fn verifying_key(&self) -> VerifyingKey;
}

#[derive(Error, Debug)]
#[error("sign error")]
pub struct SigningError;

// For now only test config because we only want to support ed25519 for software signing.
// Hardware signers usually must use P256, software signers can use ed25519.
#[cfg(test)]
impl SigningKey for p256::ecdsa::SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SigningError> {
        use p256::ecdsa::signature::Signer;
        let sig: p256::ecdsa::Signature = self.try_sign(message).map_err(|_| SigningError)?;
        Ok(Signature::P256(sig.normalize_s().to_bytes().into()))
    }

    fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey::P256(
            self.verifying_key()
                .to_sec1_point(true)
                .as_bytes()
                .try_into()
                .expect("expected 33 bytes"),
        )
    }
}
