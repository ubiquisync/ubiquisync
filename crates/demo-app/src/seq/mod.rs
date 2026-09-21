use ubiquisync_core::ids::PeerId;

pub enum Op {
    PeerAlias(PeerId),
    Insert {
        left: Option<CharId>,
        right: Option<CharId>,
        content: String,
    },
    Delete(Vec<DeleteSpan>),
}

pub struct InsertId {
    pub peer_alias: u64,
    pub idx: u64,
    pub short_hash: u32,
}

pub struct CharId {
    pub insert: InsertId,
    pub offset: u32,
}

pub struct DeleteSpan {
    pub char: CharId,
    pub len: u32,
}
