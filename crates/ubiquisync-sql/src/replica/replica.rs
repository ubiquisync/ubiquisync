use ubiquisync_core::{
    crypto::credentials::Credentials,
    hlc::HlcService,
    ids::{LogId, PeerId},
};

use crate::{db::Db, hlc_storage::SqlHlcStorage, replica::stream_lock::KeyedLock};

#[allow(dead_code)]
pub struct Replica<R> {
    pub(crate) self_id: PeerId,
    pub(crate) self_db_id: i64,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: HlcService<SqlHlcStorage>,
    pub(crate) stream_locks: KeyedLock<LogId>,
}
