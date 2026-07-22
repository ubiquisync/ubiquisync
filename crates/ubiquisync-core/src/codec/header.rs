pub struct Header {
    pub server_mode: bool,
    pub encryption_info: Option<EncryptionInfo>,
    // do we maybe need compression info here in header rather than as part of file extension? if we have encryption then we should compress before encrypting right?
}

pub struct EncryptionInfo {
    pub key_fingeprint: [u8; 16],
    pub nonce: [u8; 24],
    pub count: u32,
}
