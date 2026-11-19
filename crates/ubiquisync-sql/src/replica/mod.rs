mod branches;
mod commit;
mod commit_retry;
mod config;
mod deps;
mod exec;
mod fork;
mod fs;
mod fs_read;
mod fs_sync_schema;
mod fs_write;
mod hlc;
mod ingest;
mod init;
mod keyed_lock;
pub mod p2p;
mod peers;
mod query;
mod schema;
mod segment;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use config::{CommitRetryConfig, PackSyncConfig, ReplicaConfig};
pub use deps::{PackRemoteProviders, ReplicaDeps};
pub use hlc::HlcError;
pub use init::InitError;

use futures::lock::Mutex;
use std::sync::atomic::AtomicU64;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{task, time};
use tokio_util::sync::CancellationToken;

use crate::{
    db::Db,
    replica::{branches::LogKey, keyed_lock::KeyedLock},
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
    /// The last `branches.changed_at` stamp handed out, see `next_changed_at`.
    pub(crate) last_changed_at: AtomicU64,
    pub(crate) log_locks: KeyedLock<LogKey>,
    pub(crate) pack_remote_providers: PackRemoteProviders,
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

/// The timer for a background loop that does one round of work per tick.
pub(crate) fn poll_interval(period: Duration) -> time::Interval {
    let mut interval = time::interval(period);
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    interval
}

/// Waits for the next tick (the first one fires right away). Returns false once the replica
/// is shutting down.
pub(crate) async fn tick_or_cancelled(
    interval: &mut time::Interval,
    cancel: &CancellationToken,
) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = interval.tick() => true,
    }
}
