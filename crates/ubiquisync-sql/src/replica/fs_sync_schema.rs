use num_enum::{IntoPrimitive, TryFromPrimitive};

use crate::{def_table, def_table_with_auto_id, enum_col_repr};

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
    pack_id: Vec<u8>,
    end_seq: u64,
) => {
    // status: super::PackReadStatus,
    generation: i64,
    parents: Vec<u8>,
    first_seen: i64,
    retries: i64,
    next_retry: i64,
});

def_table!(pack_write_state as __pack_write_state (
    remote_id: i64, // TODO ref remotes
    topic_id: i64, // TODO ref topics
) => {
    dir_snapshot: Vec<u8>,
    tips: Vec<u8>,
    pending: Vec<u8>,
});

#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive)]
#[repr(i64)]
pub enum PackReadStatus {
    Blocked = 0,
    Consumed = 1,
}

enum_col_repr!(PackReadStatus);
