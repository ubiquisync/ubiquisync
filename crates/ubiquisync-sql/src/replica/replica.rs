use ubiquisync_core::{crypto::credentials::Credentials, hlc::HlcService, ids::PeerId};

use crate::{db::Db, hlc_storage::SqlHlcStorage};

pub struct Replica<R> {
    pub(crate) self_id: PeerId,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: HlcService<SqlHlcStorage>,
}
