use crate::crypto::{Key256Fingerprint, KeyWrap};

pub trait AuthController {
    fn resolve_key_wrap(
        &self,
        fingerprint: Key256Fingerprint,
    ) -> Result<Option<KeyWrap>, StoreError>;
    fn authorize_expunge(&self);
}

pub enum StoreError {
    Unavailable,
}
