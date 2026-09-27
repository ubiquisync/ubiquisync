use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use tokio::time;
use ubiquisync_core::pack::{PackStore, PackStoreError, Topic};

use crate::{
    db::{DbError, sea_query::select_cols},
    reducer::Reducer,
    replica::{Replica, ReplicaInner, fs_sync_schema::pack_read_state},
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
        for peer in store.list_topic_peers(topic).await? {
            let packs = store.list_packs(topic, &peer).await?;
            let read_state = select_cols::<(
                pack_read_state::PackId,
                pack_read_state::EndSeq,
                pack_read_state::Generation,
                pack_read_state::Parents,
                pack_read_state::FirstSeen,
                pack_read_state::Retries,
                pack_read_state::NextRetry,
            )>(
                self.db.as_ref(),
                Query::select()
                    .from(pack_read_state::Table)
                    .and_where(Expr::column(pack_read_state::RemoteId).eq(remote_id))
                    .and_where(Expr::column(pack_read_state::TopicId).eq(todo!()))
                    .and_where(Expr::column(pack_read_state::PeerId).eq(todo!())),
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ProcessPackRemoteError {
    #[error("pack store error: {0}")]
    Store(#[from] PackStoreError),
    #[error("db error: {0}")]
    Db(#[from] DbError),
}
