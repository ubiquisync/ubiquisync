use crate::{crypto::PubKey, hlc::Timestamp, uuid::Uuid};

pub struct InitEntry {
    pub version: u16,
    pub timestamp: Timestamp,
    pub device_name: String,
    pub app_magic: Vec<u8>,
    pub signing_key: PubKey,
    pub encryption_key: Option<PubKey>, // will be None if not encrypted
    pub workspace_id: Option<Uuid>,     // do we need user id to be declared at genesis?
    pub server: bool,
}
