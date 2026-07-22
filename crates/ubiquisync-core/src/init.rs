use crate::{crypto::PubKey, hlc::Timestamp, uuid::Uuid};

pub struct InitEntry {
    pub timestamp: Timestamp,
    pub device_name: String,
    pub server: bool,
    pub app_magic: Vec<u8>,
    pub signing_key: PubKey,
    pub encryption_key: Option<PubKey>,
    pub workspace_id: Option<Uuid>,
}
