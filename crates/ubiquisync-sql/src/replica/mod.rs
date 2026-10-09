mod commit;
mod config;
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
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use config::{PackSyncConfig, ReplicaConfig};
pub use hlc::HlcError;
pub use init::InitError;

use futures::lock::Mutex;
use std::sync::atomic::AtomicU64;
use std::{collections::HashMap, sync::Arc};
use tokio::task;
use tokio_util::sync::CancellationToken;

use crate::{
    db::Db,
    replica::{stream_lock::KeyedLock, streams::StreamLog},
};
use ubiquisync_core::init::InitEntry;
use ubiquisync_core::{
    crypto::{CipherKeyResolver, credentials::Credentials},
    ids::AppId,
    pack::PackStore,
};

#[allow(dead_code)]
pub struct Replica<R> {
    pub(crate) inner: Arc<ReplicaInner<R>>,
    pub(crate) tasks: task::JoinSet<()>,
    pub(crate) cancel: CancellationToken,
}

pub(crate) struct ReplicaInner<R> {
    pub(crate) app_id: AppId,
    // TODO: we can replace self_init & self_db_id with self_info: PeerInfo
    pub(crate) self_init: InitEntry,
    pub(crate) self_db_id: i64,
    pub(crate) credentials: Box<dyn Credentials>,
    pub(crate) db: Box<dyn Db>,
    pub(crate) reducer: R,
    pub(crate) hlc: AtomicU64,
    pub(crate) stream_locks: KeyedLock<StreamLog>,
    pub(crate) pack_remotes: Mutex<HashMap<i64, Arc<PackStore>>>,
    pub(crate) key_resolver: Arc<dyn CipherKeyResolver>,
    pub(crate) config: ReplicaConfig,
}

impl<R> Replica<R> {
    /// Gracefully shuts down the replica, waiting for all tasks to finish before returning.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        self.tasks.join_all().await;
    }
}
