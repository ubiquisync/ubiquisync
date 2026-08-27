use ubiquisync_core::{
    crypto::Hash256,
    ids::{LogId, PeerId},
};

use crate::{db::DbError, replica::replica::Replica};

impl Replica {
    async fn lookup_leaf(&self, log_id: &LogId, idx: u64) -> Result<Vec<LeafCandidate>, DbError> {
        todo!()
    }
}

struct LeafCandidate {
    leaf_hash: Hash256,
    branch_id: Vec<u8>,
}
