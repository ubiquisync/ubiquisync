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

    pub fn wrap_key(&self, key: &Key256) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use aes_gcm_siv::aead::Generate;
    use test_strategy::proptest;

    #[cfg(test)]
    use crate::crypto::PubKey;
    use crate::crypto::Signature;

    #[proptest(cases = 5)] // we don't need that many cases here
    fn test_ed25519_verify_signature(
        secret_key: [u8; ed25519_dalek::SECRET_KEY_LENGTH],
        msg: Vec<u8>,
    ) {
        use ed25519_dalek::Signer;

        let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_key);
        let pubkey = PubKey::Ed25519(signing_key.verifying_key().to_bytes());
        let sig = signing_key.try_sign(&msg).unwrap();
        let sig = Signature::Ed25519(sig.to_bytes());
        pubkey.verify_signature(&msg, &sig).unwrap();
    }

    #[proptest(cases = 5)]
    fn test_p256_verify_signature(msg: Vec<u8>) {
        use crypto_common::Generate;
        use p256::ecdsa::signature::Signer;

        let signing_key = p256::ecdsa::SigningKey::generate();
        let pubkey = PubKey::P256(
            signing_key
                .verifying_key()
                .to_sec1_point(true)
                .as_bytes()
                .try_into()
                .unwrap(),
        );
        let sig: p256::ecdsa::Signature = signing_key.try_sign(&msg).unwrap();
        let sig = Signature::P256(sig.to_bytes().into());
        pubkey.verify_signature(&msg, &sig).unwrap();
    }

    // TODO sad path tests
}
