use crate::{crypto::PubKey, uuid::Uuid};

pub struct EntryCoordinates {
    pub peer: Uuid,
    pub container: Uuid,
    pub entry_idx: u64,
    pub op_idx: Option<u64>,
}

pub trait KeyRing {
    fn create_key(key_type: KeyType) -> Uuid;
    fn wrap_key(key_fingerprint: Uuid, pub_key: PubKey) -> Vec<u8>;
    fn store_key(key_fingerprint: Uuid, key_type: KeyType, key: &[u8]);
    fn encrypt(key_fingerprint: Uuid, coordinates: &EntryCoordinates, payload: &[u8]) -> Vec<u8>;
    fn decrypt(key_fingerprint: Uuid, coordinates: &EntryCoordinates, payload: &[u8]) -> Vec<u8>;
}

pub enum KeyType {}

// The index of all the wraps that have been shared with this device (so they can be unwrapped in memory)
pub trait EncryptionKeyIndex {
    fn get_wrap(key_fingerprint: Uuid) -> Vec<u8>;
}
