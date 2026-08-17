use crate::{
    codec::writer::Writer,
    crypto::{EncapsulationKey, VerifyingKey},
    hlc::Timestamp,
    ids::PeerId,
};

pub struct InitEntry {
    pub app_magic: Vec<u8>,
    pub timestamp: Timestamp,
    pub server: bool,
    pub signing_key: VerifyingKey,
    pub encryption_key: EncapsulationKey,
    pub workspace_id: Option<PeerId>, // if the peer is joining an existing worksp
}

impl InitEntry {
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_len_prefixed(&self.app_magic);
        writer.write_le_u64(self.timestamp.raw());
        writer.write_byte(self.server as u8);
        self.signing_key.encode(writer);
        self.encryption_key.encode(writer);
        if let Some(workspace_id) = self.workspace_id {
            writer.write_byte(1);
            writer.write_array(&workspace_id.0);
        } else {
            writer.write_byte(0);
        }
    }
}
