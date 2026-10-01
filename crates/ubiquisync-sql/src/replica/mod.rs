mod exec;
mod fs_sync_schema;
mod hlc;
mod init;
mod query;
mod schema;
mod stream_lock;

use std::sync::atomic::AtomicU64;

pub use hlc::HlcError;
pub use init::InitError;

use ubiquisync_core::{
    crypto::credentials::Credentials,
    ids::{LogId, PeerId},
};

use crate::{db::Db, replica::stream_lock::KeyedLock};

#[allow(dead_code)]
pub struct Replica<R> {
    pub(crate) self_id: PeerId,
    pub(crate) self_db_id: i64,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: AtomicU64,
    pub(crate) stream_locks: KeyedLock<LogId>,
}
