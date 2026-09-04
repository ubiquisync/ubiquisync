use crate::{def_table, def_table_with_auto_id};

def_table_with_auto_id!(remotes (id) => {});
def_table_with_auto_id!(topics (id) => {
    dirty: bool,
});

def_table!(published (
    remote_id: i64, // TODO ref remotes
    stream_id: i64, // TODO ref streams
) => {
    published_size: u64 // TODO default 0
});

def_table!(packs_cursor (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
    peer_id: [u8;32]
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});

def_table!(writer_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});
