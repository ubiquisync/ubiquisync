use thiserror::Error;

use crate::{
    codec::{Reader, Writer},
    crypto::{CryptoDecodeError, SIG_ALGO_ED25519, SIG_ALGO_P256, Signature},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
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
                use p256::ecdsa::signature::Verifier;
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
                verifying_key
                    .verify(message, &sig)
                    .map_err(|_| SignatureVerificationError::SignatureVerificationFailed)?;
            }
        }
        Ok(())
    }

    pub fn encode(&self, writer: &mut Writer) {
        match self {
            VerifyingKey::Ed25519(key) => {
                writer.write_byte(SIG_ALGO_ED25519);
                writer.write_array(key);
            }
            VerifyingKey::P256(key) => {
                writer.write_byte(SIG_ALGO_P256);
                writer.write_array(key);
            }
        }
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, CryptoDecodeError> {
        Ok(match reader.read_byte()? {
            SIG_ALGO_ED25519 => VerifyingKey::Ed25519(reader.read_array()?),
            SIG_ALGO_P256 => VerifyingKey::P256(reader.read_array()?),
            n => return Err(CryptoDecodeError::UnknownAlgorithm(n)),
        })
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use test_strategy::proptest;

    use crate::crypto::SigningKey;
    #[cfg(test)]
    use crate::crypto::ed25519::Ed25519SigningKey;

    #[proptest(cases = 5)] // we don't need that many cases here
    fn test_ed25519_verify_signature(
        secret_key: [u8; ed25519_dalek::SECRET_KEY_LENGTH],
        msg: Vec<u8>,
    ) {
        let signing_key = Ed25519SigningKey::new(SecretBox::new(Box::new(secret_key)));
        let verifying_key = signing_key.verifying_key();
        let sig = signing_key.sign(&msg).unwrap();
        verifying_key.verify_signature(&msg, &sig).unwrap();
    }

    #[proptest(cases = 5)]
    fn test_p256_verify_signature(msg: Vec<u8>) {
        use crypto_common::Generate;

        let signing_key = p256::ecdsa::SigningKey::generate();
        let verifying_key = SigningKey::verifying_key(&signing_key);
        let sig = signing_key.sign(&msg).unwrap();
        verifying_key.verify_signature(&msg, &sig).unwrap();
    }

    // TODO sad path tests
}
