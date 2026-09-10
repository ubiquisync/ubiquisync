mod exec;
mod fs_read;
mod fs_sync_schema;
mod fs_write;
mod init;
mod peers;
mod query;
mod schema;
mod stream_lock;
mod streams;

pub use init::InitError;

use ubiquisync_core::{
    crypto::credentials::Credentials,
    hlc::HlcService,
    ids::{AppId, PeerId},
};

use crate::{
    db::Db,
    hlc_storage::SqlHlcStorage,
    replica::{stream_lock::KeyedLock, streams::StreamLog},
};

#[allow(dead_code)]
pub struct Replica<R> {
    pub(crate) app_id: AppId,
    pub(crate) self_id: PeerId,
    pub(crate) self_db_id: i64,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: HlcService<SqlHlcStorage>,
    pub(crate) stream_locks: KeyedLock<StreamLog>,
}
