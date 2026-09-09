use security_framework::{
    access_control::{ProtectionMode, SecAccessControl},
    item::Location,
    key::{Algorithm, GenerateKeyOptions, KeyType, SecKey, Token},
};
use security_framework_sys::access_control::kSecAccessControlPrivateKeyUsage;
use thiserror::Error;

use crate::crypto::{SigningError, SigningKey, VerifyingKey};

pub struct AppleP256SigningKey {
    key: SecKey,
    verifying_key: VerifyingKey,
}

#[derive(Error, Debug)]
pub enum AppleKeyError {
    #[error("other")]
    Other,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct AppleP256SigningKeyOptions {
    /// This should only be set when there is no secure enclave available.
    pub disable_secure_enclave: bool,
}

impl AppleP256SigningKey {
    pub fn new(label: &str, opts: AppleP256SigningKeyOptions) -> Result<Self, AppleKeyError> {
        let mut gen_opts = GenerateKeyOptions::default();
        gen_opts.set_label(label);
        if opts.disable_secure_enclave {
            gen_opts.set_token(Token::Software);
        } else {
            gen_opts.set_token(Token::SecureEnclave);
        }
        gen_opts.set_location(Location::DataProtectionKeychain);
        let acc_ctl = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
            kSecAccessControlPrivateKeyUsage,
        )
        .map_err(|_| AppleKeyError::Other)?;
        gen_opts.set_access_control(acc_ctl);
        // TODO what to set for access control??
        Self::new_from_gen_opts(gen_opts)
    }

    fn new_from_gen_opts(mut gen_opts: GenerateKeyOptions) -> Result<Self, AppleKeyError> {
        gen_opts.set_key_type(KeyType::ec_sec_prime_random());
        gen_opts.set_size_in_bits(256); // TODO is this necesary even?
        let key = SecKey::new(&gen_opts).map_err(|_| AppleKeyError::Other)?; // TODO errors!!
        Self::wrap(key)
    }

    fn wrap(key: SecKey) -> Result<Self, AppleKeyError> {
        let pubkey = key.public_key().ok_or(AppleKeyError::Other)?;
        let ansi_x963bytes = pubkey
            .external_representation()
            .ok_or(AppleKeyError::Other)?;
        let verifying_key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&ansi_x963bytes)
            .map_err(|_| AppleKeyError::Other)?;
        let verifying_key = VerifyingKey::P256(
            verifying_key
                .to_sec1_point(true)
                .as_bytes()
                .try_into()
                .expect("expected 33 bytes"),
        );
        Ok(Self { key, verifying_key })
    }
}

impl SigningKey for AppleP256SigningKey {
    fn sign(&self, message: &[u8]) -> Result<crate::crypto::Signature, super::SigningError> {
        let sig = self
            .key
            .create_signature(Algorithm::ECDSASignatureMessageX962SHA256, message)
            .map_err(|_| SigningError)?; // TODO handle apple interaction errors
        let sig = p256::ecdsa::Signature::from_der(&sig).map_err(|_| SigningError)?; // TODO do we need to handle conversion error?
        Ok(crate::crypto::Signature::P256(
            sig.normalize_s().to_bytes().into(),
        ))
    }

    fn verifying_key(&self) -> crate::crypto::VerifyingKey {
        self.verifying_key
    }
}

#[cfg(test)]
mod test {
    use security_framework::key::GenerateKeyOptions;
    use test_strategy::proptest;

    use crate::crypto::{SigningKey, apple::AppleP256SigningKey};

    #[proptest(cases = 1)]
    fn test_apple_signing_key(msg: Vec<u8>) {
        let key = AppleP256SigningKey::new_from_gen_opts(GenerateKeyOptions::default()).unwrap();
        let sig = key.sign(&msg).unwrap();
        key.verifying_key().verify_signature(&msg, &sig).unwrap();
    }
}
