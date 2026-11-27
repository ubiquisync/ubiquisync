use ubiquisync_core::{
    crypto::credentials::Credentials,
    event::{EventBus, Subscribe, event_bus},
    ids::{AppId, ContainerId},
    uuid::Uuid,
};
use ubiquisync_sql::{
    Exec, ExecError, SqlQueryStore,
    db::{Db, DbError, DbRow, DbValue},
    dialect::SqlDialect,
    replica::{
        InitError as ReplicaInitError, PackRemoteInitError, PackRemoteProviders, RemoteId, Replica,
        ReplicaConfig, ReplicaDeps, SyncEvents, SyncStatus,
    },
};

use crate::{
    error::TablesInitError, op::Op, reducer::Reducer, schema::TableSchema, watch::ChangeEvent,
};

pub struct StoreImpl {
    replica: Replica<Reducer>,
    event_bus: EventBus<ChangeEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreInitError {
    #[error("tables init error: {0}")]
    TablesInit(#[from] TablesInitError),
    #[error("replica init error: {0}")]
    ReplicaInit(#[from] ReplicaInitError),
}

impl StoreImpl {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        app_magic: AppId,
        container_id: ContainerId,
        credentials: Box<dyn Credentials>,
        prefix: &str,
        tables: &[TableSchema],
        db: Box<dyn Db>,
        pack_remote_providers: PackRemoteProviders,
        replica_config: ReplicaConfig,
    ) -> Result<Self, StoreInitError> {
        let (event_handler, event_bus) = event_bus();
        let reducer =
            Reducer::new(container_id, prefix, tables, db.as_ref(), event_handler).await?;
        let deps = ReplicaDeps {
            db,
            reducer,
            credentials,
            pack_remote_providers,
        };
        let replica = Replica::new(app_magic, deps, replica_config).await?;
        Ok(Self { replica, event_bus })
    }

    /// Attaches a sync folder through one of the providers passed to [`new`](Self::new).
    /// The db remembers it, so it is attached again on the next start.
    pub async fn add_remote(
        &self,
        provider: &str,
        config: &str,
    ) -> Result<RemoteId, PackRemoteInitError> {
        self.replica.add_remote(provider, config).await
    }

    /// Stops the background tasks and waits for them to finish.
    pub async fn shutdown(self) {
        self.replica.shutdown().await
    }

    /// A snapshot of the sync state, see [`Replica::status`].
    pub async fn status(&self) -> Result<SyncStatus, DbError> {
        self.replica.status().await
    }

    /// Sync events from now on, see [`Replica::watch`].
    pub fn watch(&self) -> SyncEvents {
        self.replica.watch()
    }

    /// Pauses or resumes sync with every folder, see [`Replica::set_sync_paused`].
    pub fn set_sync_paused(&self, paused: bool) {
        self.replica.set_sync_paused(paused)
    }

    pub fn sync_paused(&self) -> bool {
        self.replica.sync_paused()
    }
}

#[cfg(feature = "inspect")]
impl StoreImpl {
    /// See [`Replica::devices`].
    pub async fn devices(&self) -> Result<Vec<ubiquisync_sql::replica::DeviceInfo>, DbError> {
        self.replica.devices().await
    }

    /// See [`Replica::entries`]. Decode op bytes with [`decode_any`](crate::codec::decode_any).
    pub async fn entries(
        &self,
        log: ubiquisync_core::ids::LogId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<ubiquisync_sql::replica::EntryView>, ubiquisync_sql::replica::InspectError>
    {
        self.replica.entries(log, range).await
    }

    /// See [`Replica::remote_files`].
    pub async fn remote_files(
        &self,
        remote: RemoteId,
    ) -> Result<Vec<ubiquisync_sql::replica::RemoteFile>, ubiquisync_sql::replica::InspectError>
    {
        self.replica.remote_files(remote).await
    }
}

#[async_trait::async_trait]
impl Exec<Op> for StoreImpl {
    async fn exec(&self, server_user_id: Option<Uuid>, op: Op) -> Result<(), ExecError> {
        self.replica.exec(server_user_id, op).await
    }
}

#[async_trait::async_trait]
impl SqlQueryStore for StoreImpl {
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        self.replica.query(sql, params).await
    }

    fn dialect(&self) -> SqlDialect {
        self.replica.dialect()
    }
}

impl Subscribe<ChangeEvent> for StoreImpl {
    fn subscribe(
        &self,
        target: <ChangeEvent as ubiquisync_core::event::RoutableEvent>::Target,
    ) -> ubiquisync_core::event::Subscription<ChangeEvent> {
        self.event_bus.subscribe(target)
    }
}
