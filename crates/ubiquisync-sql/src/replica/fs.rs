use sea_query::{Expr, ExprTrait, OnConflict, Query};
use thiserror::Error;
use tokio::time;
use ubiquisync_core::{
    hlc::wall_ms,
    pack::{PackStore, PackStoreError, Topic},
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols, select_cols},
    },
    reducer::Reducer,
    replica::{
        Replica, ReplicaInner,
        fs_sync_schema::{pack_read_state, topics},
        peers::PeerResolveError,
    },
};

impl<R: Reducer> Replica<R> {
    pub(crate) fn start_process_packs(&mut self) {
        let inner = self.inner.clone();
        let cancel_token = self.cancel.clone();
        self.tasks.spawn(async move {
            const PACK_POLL_INTERVAL: time::Duration = time::Duration::from_secs(2);
            let mut interval = time::interval(PACK_POLL_INTERVAL);
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        break;
                    }
                    _ = interval.tick() => {
                        let _ = inner.process_packs().await;
                    }
                }
            }
        });
    }
}

impl<R: Reducer> ReplicaInner<R> {
    /// A single round of pack processing, read, then write.
    async fn process_packs(&self) {
        for (remote_id, store) in self.pack_remotes.iter() {
            match self.process_pack_remote(*remote_id, store).await {
                Ok(_) => {}
                Err(_) => todo!(),
            }
        }
    }

    async fn process_pack_remote(
        &self,
        remote_id: u64,
        store: &PackStore,
    ) -> Result<(), ProcessPackRemoteError> {
        // for MVP we only subscribe to the default topic
        self.process_topic(remote_id, store, &Topic::default())
            .await
    }

    async fn process_topic(
        &self,
        remote_id: u64,
        store: &PackStore,
        topic: &Topic,
    ) -> Result<(), ProcessPackRemoteError> {
        let topic_id = self.resolve_topic_id(topic).await?;
        for peer in store.list_topic_peers(topic).await? {
            let peer_info = self.resolve_or_init_peer(&peer, store).await?;
            let packs = store.list_packs(topic, &peer).await?;
            let (read_state,) = select_cols::<(pack_read_state::State,)>(
                self.db.as_ref(),
                Query::select()
                    .from(pack_read_state::Table)
                    .and_where(Expr::column(pack_read_state::RemoteId).eq(remote_id))
                    .and_where(Expr::column(pack_read_state::TopicId).eq(topic_id))
                    .and_where(Expr::column(pack_read_state::PeerId).eq(peer_info.db_id)),
            )
            .await?
            .one()?
            .unwrap_or_default();

            let read_plan = read_state.prepare_read(wall_ms(), &packs);
        }
        Ok(())
    }

    async fn resolve_topic_id(&self, topic: &Topic) -> Result<i64, DbError> {
        let (id,) = insert_cols::<(topics::Topic,), (topics::Id,)>(
            self.db.as_ref(),
            (topic.dir(),),
            Query::insert()
                .into_table(topics::Table)
                .on_conflict(OnConflict::column(topics::Topic).do_nothing().to_owned())
                .returning(Query::returning().column(topics::Id)),
        )
        .await?
        .exactly_one()?;
        Ok(id)
    }
}

#[derive(Error, Debug)]
pub enum ProcessPackRemoteError {
    #[error("pack store error: {0}")]
    Store(#[from] PackStoreError),
    #[error("peer resolve error: {0}")]
    Peer(#[from] PeerResolveError),
    #[error("db error: {0}")]
    Db(#[from] DbError),
}
