use ubiquisync_core::ids::PeerId;

pub struct EntryId<const N: usize> {
    pub peer_id: PeerId,
    pub entry_index: u64,
    pub hash: [u8; N],
}

pub struct WireEntryId<const N: usize> {
    pub peer_alias: u64,
    pub entry_index: u64,
    pub hash: [u8; N],
}
