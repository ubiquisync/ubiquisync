use ed25519_dalek::Verifier;
use thiserror::Error;

use crate::crypto::{Key256, Signature};

pub enum PubKey {
    Ed25519([u8; 32]),
    P256([u8; 33]),
}

#[derive(Error, Debug)]
pub enum SignatureVerificationError {
    #[error("invalid key")]
    InvalidKey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("signature verification failed")]
    SignatureVerificationFailed,
}

impl PubKey {
    pub fn verify_signature(
        &self,
        message: &[u8],
        signature: &Signature,
    ) -> Result<(), SignatureVerificationError> {
        match self {
            PubKey::Ed25519(key) => {
                let sig = match signature {
                    Signature::Ed25519(sig) => sig,
                    Signature::P256(_) => return Err(SignatureVerificationError::InvalidSignature),
                };
                let sig = ed25519_dalek::Signature::from_bytes(sig);
                let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(key)
                    .map_err(|_| SignatureVerificationError::InvalidKey)?;
                verifying_key
                    .verify_strict(message, &sig)
                    .map_err(|_| SignatureVerificationError::SignatureVerificationFailed)?;
            }
            PubKey::P256(key) => {
                let sig = match signature {
                    Signature::P256(sig) => sig,
                    Signature::Ed25519(_) => {
                        return Err(SignatureVerificationError::InvalidSignature);
                    }
                };
                let sig = p256::ecdsa::Signature::from_bytes(sig.into())
                    .map_err(|_| SignatureVerificationError::InvalidSignature)?;
                let verifying_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&key[..])
                    .map_err(|_| SignatureVerificationError::InvalidKey)?;
                // TODO check that s is normalized
                verifying_key
                    .verify(message, &sig)
                    .map_err(|_| SignatureVerificationError::SignatureVerificationFailed)?;
            }
        }
        Ok(())
    }

    pub fn wrap_key(&self, key: &Key256) {
        todo!()
    }
}
