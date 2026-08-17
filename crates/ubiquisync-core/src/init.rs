use crate::{crypto::VerifyingKey, hlc::Timestamp, uuid::Uuid};

pub struct InitEntry {
    pub version: u16,
    pub timestamp: Timestamp,
    pub device_name: String,
    pub app_magic: Vec<u8>,
    pub signing_key: VerifyingKey,
    pub encryption_key: Option<VerifyingKey>, // will be None if not encrypted
    pub workspace_id: Option<Uuid>,           // do we need user id to be declared at genesis?
    pub server: bool,
}
