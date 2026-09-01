use ubiquisync_core::{crypto::credentials::Credentials, ids::PeerId};

use crate::db::Db;

pub struct Replica<R> {
    pub(crate) self_id: PeerId,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
}
