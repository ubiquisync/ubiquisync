use ed25519_dalek::Verifier;
use thiserror::Error;

pub enum PubKey {
    Ed25519([u8; 32]),
    P256([u8; 33]),
}

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error("invalid key")]
    InvalidKey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("signature verification failed")]
    SignatureVerificationFailed,
}

impl PubKey {
    pub fn verify_signature(&self, message: &[u8], signature: &[u8]) -> Result<(), VerifyError> {
        match self {
            PubKey::Ed25519(key) => {
                let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(key)
                    .map_err(|_| VerifyError::InvalidKey)?;
                let sig = ed25519_dalek::Signature::from_slice(signature)
                    .map_err(|_| VerifyError::InvalidSignature)?;
                verifying_key
                    .verify_strict(message, &sig)
                    .map_err(|_| VerifyError::SignatureVerificationFailed)?;
            }
            PubKey::P256(key) => {
                let verifying_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&key[..])
                    .map_err(|_| VerifyError::InvalidKey)?;
                let sig = p256::ecdsa::Signature::from_slice(signature)
                    .map_err(|_| VerifyError::InvalidSignature)?;
                // TODO check that s is normalized
                verifying_key
                    .verify(message, &sig)
                    .map_err(|_| VerifyError::SignatureVerificationFailed)?;
            }
        }
        Ok(())
    }
}
