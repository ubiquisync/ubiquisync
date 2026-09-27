use std::{
    collections::{HashMap, HashSet},
    range::Range,
};

use ubiquisync_core::{
    codec::{ReadError, Reader, WriteError, Writer},
    pack::{PackFileId, PackRef},
};

use crate::{codeable_col_repr, def_table, def_table_with_auto_id};

def_table_with_auto_id!(remotes as __replica_remotes (id) => {});
def_table_with_auto_id!(topics as __pack_topics (id) => {
    topic: String,
});

def_table!(topic_state as __pack_topic_state (remote_id: i64, topic_id: i64) => {
    dirty: bool,
});

def_table!(published as __pack_published (
    remote_id: i64, // TODO ref remotes
    stream_id: i64, // TODO ref streams
) => {
    published_size: u64 // TODO default 0
});

def_table!(pack_read_state as __pack_read_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
    peer_id: i64, // TODO ref peers
) => {
    state: super::PackReadState,
});

def_table!(pack_write_state as __pack_write_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});

codeable_col_repr!(PackReadState);

#[derive(Debug, Clone, Default)]
pub struct PackReadState {
    pub consumed: HashSet<PackRef>,
    pub blocked: HashMap<PackRef, BlockedPackInfo>,
}

#[derive(Debug, Clone)]
pub struct BlockedPackInfo {
    pub file: PackFileId,
    pub parents: HashSet<PackRef>,
    pub read_timestamps: Range<u64>,
    pub attempts: u64,
}

impl PackReadState {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WriteError> {
        todo!()
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError> {
        todo!()
    }
}
