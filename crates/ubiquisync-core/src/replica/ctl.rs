use crate::crypto::{Key256Fingerprint, KeyWrap};

pub trait AuthController {
    fn resolve_key_wrap(&self, fingerprint: &Key256Fingerprint) -> Option<KeyWrap>;
    fn authorize_expunge(&self);
}
