use ubiquisync_core::ids::PeerId;

pub struct EntryId<const N: usize = 4> {
    pub peer_alias: u64,
    pub entry_index: u64,
    pub hash: [u8; N],
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ElementId<Id = EntryId> {
    pub op_id: Id,
    pub index: u64,
}

pub struct PeerAlias {
    pub peer_id: PeerId,
    pub alias: u64,
}
