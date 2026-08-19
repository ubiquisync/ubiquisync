use crate::{
    crypto::{Key256, Key256Fingerprint, kem::DecapsulationKey},
    replica::AuthController,
};

pub(crate) struct CipherKeyResolver<'a> {
    ctl: &'a dyn AuthController,
    decap_key: &'a dyn DecapsulationKey,
}

impl<'a> CipherKeyResolver<'a> {
    pub(crate) fn resolve(&self, fingerprint: &Key256Fingerprint) -> Option<Key256> {
        // TODO do we need an error type here or is Option enough??
        let key_wrap = self.ctl.resolve_key_wrap(fingerprint)?;
        let key = self.decap_key.unwrap_key(fingerprint, &key_wrap)?;
        Some(key)
    }
}
