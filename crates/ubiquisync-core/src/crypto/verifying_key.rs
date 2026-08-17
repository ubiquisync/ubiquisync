use thiserror::Error;

use crate::crypto::Signature;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyingKey {
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

impl VerifyingKey {
    pub fn verify_signature(
        &self,
        message: &[u8],
        signature: &Signature,
    ) -> Result<(), SignatureVerificationError> {
        match self {
            VerifyingKey::Ed25519(key) => {
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
            VerifyingKey::P256(key) => {
                use ed25519_dalek::Verifier;
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
}

#[cfg(test)]
mod tests {
    use test_strategy::proptest;

    use crate::crypto::Signature;
    use crate::crypto::SigningKey;
    use crate::crypto::VerifyingKey;

    #[proptest(cases = 5)] // we don't need that many cases here
    fn test_ed25519_verify_signature(
        secret_key: [u8; ed25519_dalek::SECRET_KEY_LENGTH],
        msg: Vec<u8>,
    ) {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_key);
        let verifying_key = signing_key.get_verifying_key();
        let sig = signing_key.sign(&msg).unwrap();
        verifying_key.verify_signature(&msg, &sig).unwrap();
    }

    #[proptest(cases = 5)]
    fn test_p256_verify_signature(msg: Vec<u8>) {
        use crypto_common::Generate;

        let signing_key = p256::ecdsa::SigningKey::generate();
        let verifying_key = signing_key.get_verifying_key();
        let sig = signing_key.sign(&msg).unwrap();
        verifying_key.verify_signature(&msg, &sig).unwrap();
    }

    // TODO sad path tests
}
