use thiserror::Error;

use crate::crypto::{Signature, VerifyingKey};

pub trait SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SignError>;
    // NOTE: named get_verifying_key just to not conflict with verifying_key which many keys define
    fn get_verifying_key(&self) -> VerifyingKey;
}

#[derive(Error, Debug)]
#[error("sign error")]
pub struct SignError;

impl SigningKey for ed25519_dalek::SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SignError> {
        use ed25519_dalek::Signer;

        let sig = self.try_sign(&message).map_err(|_| SignError)?;
        Ok(Signature::Ed25519(sig.to_bytes()))
    }

    fn get_verifying_key(&self) -> VerifyingKey {
        VerifyingKey::Ed25519(self.verifying_key().to_bytes())
    }
}

impl SigningKey for p256::ecdsa::SigningKey {
    fn sign(&self, message: &[u8]) -> Result<Signature, SignError> {
        use ed25519_dalek::Signer;
        let sig: p256::ecdsa::Signature = self.try_sign(&message).unwrap();
        Ok(Signature::P256(sig.to_bytes().into()))
    }

    fn get_verifying_key(&self) -> VerifyingKey {
        VerifyingKey::P256(
            self.verifying_key()
                .to_sec1_point(true)
                .as_bytes()
                .try_into()
                .unwrap(),
        )
    }
}
