use thiserror::Error;

use crate::{crypto::PubKey, uuid::Uuid};

#[derive(Error, Debug)]
#[error("pub keyring error")]
pub struct PubKeyRingError;

pub trait PubKeyRing {
    fn lookup_signing_key(&self, peer_id: Uuid) -> Result<Option<PubKey>, PubKeyRingError>;
    fn lookup_encryption_key(&self, peer_id: Uuid) -> Result<Option<PubKey>, PubKeyRingError>;
}
