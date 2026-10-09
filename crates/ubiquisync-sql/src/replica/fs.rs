use std::{collections::HashMap, sync::Arc};

use backon::{BackoffBuilder, ExponentialBackoff, ExponentialBuilder};
use sea_query::{Expr, ExprTrait, Func, IntoIden, OnConflict, Query};
use thiserror::Error;
use tokio::time;
use ubiquisync_core::{
    hlc::WallTime,
    log::segment::SegmentEncodeError,
    pack::{PackFileDescriptor, PackStore, PackStoreError, Topic},
};

use crate::{
    db::{
        DbBatch, DbError,
        sea_query::{build_sql, insert_cols, insert_cols_batch, select_cols},
    },
    reducer::Reducer,
    replica::{
        Replica, ReplicaInner,
        config::PackSyncConfig,
        fs_read::PackTipTracker,
        fs_sync_schema::{
            BlockedPackInfo, DEFAULT_TOPIC_ID, PackReadState, PackWriteState, pack_read_state,
            published, remotes, topics,
        },
        fs_write::PackPublishError,
        peers::PeerResolveError,
        schema::streams,
        segment::GetSegmentError,
    },
};

impl<R: Reducer> Replica<R> {
    pub async fn add_remote(
        &self,
        provider: &str,
        config: &str,
    ) -> Result<i64, PackRemoteInitError> {
        let store = self.inner.init_remote(provider, config).await?;
        let (id,) = insert_cols::<(remotes::Provider, remotes::Config), (remotes::Id,)>(
            self.inner.db.as_ref(),
            (provider.into(), config.into()),
            Query::insert().into_table(remotes::Table),
        )
        .await?
        .exactly_one()?;
        let mut remotes_guard = self.inner.pack_remotes.lock().await;
        remotes_guard.insert(id, Arc::new(store));
        Ok(id)
    }

    // TODO add remove_remote fn
    //
    pub(crate) fn start_process_packs(&mut self) {
        let inner = self.inner.clone();
        let cancel_token = self.cancel.clone();
        self.tasks.spawn(async move {
            let mut interval = time::interval(inner.config.pack_sync.poll_interval);
            interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
            let mut backoffs = HashMap::new();
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                    _ = interval.tick() => {
                        inner.process_pack_remotes(&mut backoffs).await;
                    }
                }
            }
        });
    }
}

#[derive(Debug, Error)]
pub enum PackRemoteInitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("remote error: {0}")]
    Store(#[from] PackStoreError),
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
}

impl<R: Reducer> ReplicaInner<R> {
    pub(crate) async fn init_remotes(&self) -> Result<(), DbError> {
        let remotes = select_cols::<(remotes::Id, remotes::Provider, remotes::Config)>(
            self.db.as_ref(),
            Query::select().from(remotes::Table),
        )
        .await?;

        let mut remotes_guard = self.pack_remotes.lock().await;
        for remote in remotes.iter() {
            let (id, prov, cfg) = remote?;

            match self.init_remote(prov, cfg).await {
                Ok(store) => {
                    remotes_guard.insert(id, Arc::new(store));
                }
                Err(error) => {
                    tracing::warn!(remote_id = id, provider = prov, %error, "error initializing remote");
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn init_remote(
        &self,
        prov: &str,
        cfg: &str,
    ) -> Result<PackStore, PackRemoteInitError> {
        let Some(provider) = self.config.pack_remote_providers.get(prov) else {
            return Err(PackRemoteInitError::UnknownProvider(prov.into()));
        };

        let remote = provider.init(cfg).await.map_err(PackStoreError::Remote)?;
        let store = PackStore::new(self.self_init.peer_id, remote);
        Ok(store)
    }

    /// A single round of pack processing for a remote, first read, then write.
    /// Remotes which have errors which filter up, are retried with backoff.
    #[tracing::instrument(skip_all)]
    async fn process_pack_remotes(&self, backoffs: &mut HashMap<i64, RemoteBackoff>) {
        // clone the remotes map so we don't hold the lock for the whole round
        let remotes = { self.pack_remotes.lock().await.clone() };
        for (remote_id, store) in remotes.iter() {
            if backoffs
                .get(remote_id)
                .is_some_and(|b| time::Instant::now() < b.retry_at)
            {
                continue;
            }
            match self.process_pack_remote(*remote_id, store).await {
                Ok(()) => {
                    backoffs.remove(remote_id);
                }
                Err(e) => {
                    let cfg = &self.config.pack_sync;
                    let b = backoffs
                        .entry(*remote_id)
                        .or_insert_with(|| RemoteBackoff::new(cfg));
                    let delay = b.delays.next().unwrap_or(cfg.remote_retry_max);
                    b.retry_at = time::Instant::now() + delay;
                    tracing::warn!(remote_id, error = %e, ?delay, "pack round failed for remote, backing off");
                }
            }
        }
    }

    async fn process_pack_remote(
        &self,
        remote_id: i64,
        store: &PackStore,
    ) -> Result<(), PackProcessError> {
        // for MVP we only subscribe to the default topic
        self.process_topic(remote_id, store, &Topic::default())
            .await?;

        self.update_peer_published_pending(remote_id).await?;

        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn process_topic(
        &self,
        remote_id: i64,
        store: &PackStore,
        topic: &Topic,
    ) -> Result<(), PackProcessError> {
        let topic_id = self.resolve_topic_id(topic).await?;
        for peer in store.list_topic_peers(topic).await? {
            let Some(peer_info) = self.resolve_or_init_peer(&peer, store).await? else {
                tracing::warn!(remote_id, %peer, "no peer init file, skipping peer");
                continue;
            };
            let packs = store.list_packs(topic, &peer).await?;
            let read_state = self
                .get_pack_read_state(remote_id, topic_id, peer_info.db_id)
                .await?;
            let mut read_plan =
                read_state.prepare_read(WallTime::now(), &packs, &self.config.pack_sync);

            // track tips for self, keeping the loaded tips to check for changes
            let (mut tip_tracker, loaded_tips) = if peer == self.self_init.peer_id {
                let write_state = self.get_pack_write_state(remote_id, topic_id).await?;
                (
                    Some(PackTipTracker {
                        last: read_state.clone(),
                        tips: write_state.tips.clone(),
                    }),
                    Some(write_state.tips),
                )
            } else {
                (None, None)
            };

            let mut err = None;
            for todo in read_plan.to_read.iter() {
                let pack_file_desc = PackFileDescriptor {
                    topic: topic.clone(),
                    peer_id: peer,
                    id: todo.file().clone(),
                };
                match self
                    .process_pack(
                        &peer_info,
                        &pack_file_desc,
                        remote_id,
                        store,
                        todo,
                        &mut tip_tracker,
                    )
                    .await
                {
                    Err(e) => match e {
                        PackProcessError::PackGone => { /* keep going */ }
                        PackProcessError::Blocked(bi) => {
                            // mark as blocked
                            read_plan.next.blocked.insert(todo.file().get_ref(), bi);
                        }
                        e @ (PackProcessError::Peer(_)
                        | PackProcessError::Store(_)
                        | PackProcessError::Db(_)
                        | PackProcessError::GetSegment(_)
                        | PackProcessError::Encode(_)
                        | PackProcessError::Publish(_)
                        | PackProcessError::Internal(_)) => {
                            tracing::warn!(remote_id, %peer, file = %todo.file(), error = %e, "aborting pack process round");
                            // setting err here will cause the round to stop, but will attempt to
                            // save the peer's state before returning
                            err = Some(e);
                            break;
                        }
                    },
                    Ok(()) => {
                        // mark as consumed
                        read_plan.next.consumed.insert(todo.file().clone());
                    }
                }
            }

            // only write state that changed, most rounds change nothing
            let read_state_changed = read_plan.next != read_state;
            let new_tips = tip_tracker
                .map(|t| t.tips)
                .filter(|tips| Some(tips) != loaded_tips.as_ref());
            if read_state_changed || new_tips.is_some() {
                let mut db_batch = self.db.new_batch();
                if read_state_changed {
                    self.update_pack_read_state_batch(
                        db_batch.as_mut(),
                        remote_id,
                        topic_id,
                        peer_info.db_id,
                        read_plan.next,
                    )?;
                }
                // persist writer state
                if let Some(tips) = new_tips {
                    self.update_pack_write_state_batch(
                        db_batch.as_mut(),
                        remote_id,
                        topic_id,
                        PackWriteState { tips },
                    )?;
                }
                db_batch.commit().await?;
            }

            if let Some(err) = err {
                // there was an error for this peer which aborts the pack round, so we pass it upwards
                return Err(err);
            }
        }

        // after we've read all new packs for this topic, we then publish local segments which aren't in the remote
        self.publish_topic(remote_id, topic_id, topic, store)
            .await?;

        Ok(())
    }

    pub(crate) async fn get_pack_read_state(
        &self,
        remote_id: i64,
        topic_id: i64,
        peer_db_id: i64,
    ) -> Result<PackReadState, DbError> {
        Ok(select_cols::<(pack_read_state::State,)>(
            self.db.as_ref(),
            Query::select()
                .from(pack_read_state::Table)
                .and_where(Expr::column(pack_read_state::RemoteId).eq(remote_id))
                .and_where(Expr::column(pack_read_state::TopicId).eq(topic_id))
                .and_where(Expr::column(pack_read_state::PeerId).eq(peer_db_id)),
        )
        .await?
        .one()?
        .unwrap_or_default()
        .0)
    }

    async fn resolve_topic_id(&self, topic: &Topic) -> Result<i64, DbError> {
        if topic.is_default() {
            return Ok(DEFAULT_TOPIC_ID);
        }
        if let Some((topic_id,)) = select_cols::<(topics::Id,)>(
            self.db.as_ref(),
            Query::select()
                .from(topics::Table)
                .and_where(Expr::col(topics::Topic).eq(topic.dir())),
        )
        .await?
        .one()?
        {
            Ok(topic_id)
        } else {
            let (id,) = insert_cols::<(topics::Topic,), (topics::Id,)>(
                self.db.as_ref(),
                (topic.dir(),),
                Query::insert()
                    .into_table(topics::Table)
                    // we update here if the row already exists to get the id returned if there is a concurrent insert
                    .on_conflict(
                        OnConflict::column(topics::Topic)
                            .update_column(topics::Topic)
                            .to_owned(),
                    ),
            )
            .await?
            .exactly_one()?;
            Ok(id)
        }
    }

    pub(crate) fn update_pack_read_state_batch(
        &self,
        batch: &mut dyn DbBatch,
        remote_id: i64,
        topic_id: i64,
        peer_db_id: i64,
        read_state: PackReadState,
    ) -> Result<(), DbError> {
        insert_cols_batch::<(
            pack_read_state::RemoteId,
            pack_read_state::TopicId,
            pack_read_state::PeerId,
            pack_read_state::State,
        )>(
            batch,
            (remote_id, topic_id, peer_db_id, read_state),
            Query::insert()
                .into_table(pack_read_state::Table)
                .on_conflict(
                    OnConflict::columns([
                        pack_read_state::RemoteId.into_iden(),
                        pack_read_state::TopicId.into_iden(),
                        pack_read_state::PeerId.into_iden(),
                    ])
                    .update_column(pack_read_state::State)
                    .to_owned(),
                ),
        )
    }

    // Timestamp peer streams which we hold but haven't seen published on this remote as pending
    // so we know to publish them ourselves if the grace period expires
    pub(crate) async fn update_peer_published_pending(
        &self,
        remote_id: i64,
    ) -> Result<(), DbError> {
        let now = WallTime::now();
        let (sql, params) = build_sql(
            &Query::insert()
                .into_table(published::Table)
                .columns([
                    published::RemoteId.into_iden(),
                    published::StreamId.into_iden(),
                    published::PublishedSize.into_iden(),
                    published::PendingSince.into_iden(),
                ])
                .select_from(
                    Query::select()
                        .expr(Expr::val(remote_id))
                        .column((streams::Table, streams::Id))
                        .expr(Expr::val(0))
                        .expr(Expr::val(now.as_millis()))
                        .from(streams::Table)
                        .left_join(
                            published::Table,
                            Expr::col((streams::Table, streams::Id))
                                .eq(Expr::col((published::Table, published::StreamId)))
                                // this condition belongs in the LEFT JOIN ON condition because we don't
                                // want to filter stream rows where there is no published row
                                .and(
                                    Expr::col((published::Table, published::RemoteId))
                                        .eq(remote_id),
                                ),
                        )
                        .and_where(Expr::col((published::Table, published::PendingSince)).is_null())
                        .and_where(Expr::col((streams::Table, streams::PeerId)).ne(self.self_db_id))
                        .and_where(Expr::col((streams::Table, streams::HeadSize)).gt(
                            Func::coalesce([
                                Expr::col((published::Table, published::PublishedSize)),
                                Expr::val(0),
                            ]),
                        ))
                        .take(),
                )
                .expect("valid select from")
                .on_conflict(
                    OnConflict::columns([
                        published::RemoteId.into_iden(),
                        published::StreamId.into_iden(),
                    ])
                    .update_column(published::PendingSince)
                    .to_owned(),
                )
                .take(),
            self.db.dialect(),
        )?;
        self.db.exec(&sql, &params).await?;
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum PackProcessError {
    #[error("pack store error: {0}")]
    Store(#[from] PackStoreError),
    #[error("peer resolve error: {0}")]
    Peer(#[from] PeerResolveError),
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("publish error: {0}")]
    Publish(#[from] PackPublishError),
    #[error("get segment error: {0}")]
    GetSegment(#[from] GetSegmentError),
    #[error("encode error: {0}")]
    Encode(SegmentEncodeError),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("blocked: {0:?}")]
    Blocked(BlockedPackInfo),
    #[error("pack gone")]
    PackGone,
}

pub(crate) struct RemoteBackoff {
    retry_at: time::Instant,
    delays: ExponentialBackoff,
}

impl RemoteBackoff {
    fn new(cfg: &PackSyncConfig) -> Self {
        Self {
            retry_at: time::Instant::now(),
            delays: ExponentialBuilder::new()
                .with_min_delay(cfg.remote_retry_min)
                .with_max_delay(cfg.remote_retry_max)
                .with_jitter()
                .without_max_times()
                .build(),
        }
    }
}
