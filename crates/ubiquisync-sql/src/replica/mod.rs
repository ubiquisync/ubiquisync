mod commit;
mod exec;
mod fork;
mod fs;
mod fs_read;
mod fs_sync_schema;
mod fs_write;
mod hlc;
mod ingest;
mod init;
mod peers;
mod query;
mod schema;
mod segment;
mod stream_lock;
mod streams;

use std::{collections::HashMap, sync::Arc};

use std::sync::atomic::AtomicU64;

use futures::lock::Mutex;
pub use hlc::HlcError;
pub use init::InitError;

use tokio::task;
use tokio_util::sync::CancellationToken;
use ubiquisync_core::{
    crypto::{CipherKeyResolver, credentials::Credentials},
    ids::{AppId, PeerId},
    pack::PackStore,
};

use crate::{
    db::Db,
    replica::{stream_lock::KeyedLock, streams::StreamLog},
};

#[allow(dead_code)]
pub struct Replica<R> {
    pub(crate) inner: Arc<ReplicaInner<R>>,
    pub(crate) tasks: task::JoinSet<()>,
    pub(crate) cancel: CancellationToken,
}

pub(crate) struct ReplicaInner<R> {
    pub(crate) app_id: AppId,
    pub(crate) self_id: PeerId,
    pub(crate) self_db_id: i64,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: AtomicU64,
    pub(crate) stream_locks: KeyedLock<StreamLog>,
    pub(crate) pack_remotes: Mutex<HashMap<i64, PackStore>>,
    pub(crate) key_resolver: Arc<dyn CipherKeyResolver>,
}
