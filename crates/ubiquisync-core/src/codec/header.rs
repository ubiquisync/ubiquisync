use crate::uuid::Uuid;

pub struct LogicalHeader {
    pub peer_id: Uuid,
    pub container_id: Uuid,
    pub start_idx: u64,
    pub segment_header: SegmentHeader,
}

pub struct SegmentHeader {
    pub app_magic: Vec<u8>,
    pub app_version: u16,
    pub protocol_version: u16,
    pub server_mode: bool,
    pub encryption_info: Option<EncryptionInfo>,
    pub compression: Compression,
}

pub struct EncryptionInfo {
    pub key_fingerprint: [u8; 32],
    pub nonce: [u8; 24],
    pub count: u32,
}

pub enum Compression {
    None,
    Zstd,
}
