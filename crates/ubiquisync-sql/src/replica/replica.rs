use ubiquisync_core::{crypto::credentials::Credentials, hlc::HlcService, ids::PeerId};

use crate::{db::Db, hlc_storage::SqlHlcStorage};

pub struct Replica<R> {
    pub(crate) self_id: PeerId,
    pub(crate) self_db_id: i64, // TODO: grab this at startup
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: HlcService<SqlHlcStorage>,
}
