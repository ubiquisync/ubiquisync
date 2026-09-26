use crate::{def_table, def_table_with_auto_id};

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

def_table!(packs_cursors as __pack_cursors (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
    peer_id: [u8;32]
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});

def_table!(writer_state as __pack_writer_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});
