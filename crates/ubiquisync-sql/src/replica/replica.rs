use ubiquisync_core::{crypto::credentials::Credentials, ids::PeerId};

use crate::db::Db;

pub struct Replica {
    self_id: PeerId,
    credentials: Box<dyn Credentials>,
    db: Box<dyn Db>,
}
