use std::collections::HashSet;

use crate::{
    db::{
        DbBatch, DbError, Nullable,
        sea_query::{insert_cols, insert_cols_batch, select_cols},
    },
    reducer::Reducer,
    replica::{
        ReplicaInner,
        fs_sync_schema::{DEFAULT_TOPIC_ID, PackWriteState, published, topic_state},
        ingest::IngestSource,
        schema::{branches, containers, peers, segments},
    },
};
use sea_query::{
    Expr, ExprTrait, Func, InsertStatement, IntoColumnRef, IntoIden, OnConflict, Order, Query,
};
use thiserror::Error;
use ubiquisync_core::{
    hlc::WallTime,
    ids::{ContainerId, LogId, PeerId},
    log::LogHashContext,
    pack::{
        PackBuildError, PackBuilder, PackFileDescriptor, PackFileId, PackSignError, PackStore,
        PackStoreError, Topic,
    },
};

#[derive(Error, Debug)]
pub(crate) enum PackPublishError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("pack build error: {0}")]
    Build(#[from] PackBuildError),
    #[error("pack sign error: {0}")]
    Sign(#[from] PackSignError),
    #[error("pack store error: {0}")]
    Store(#[from] PackStoreError),
}

impl<R: Reducer> ReplicaInner<R> {
    #[tracing::instrument(skip_all)]
    pub(crate) async fn publish_topic(
        &self,
        remote_id: i64,
        topic_id: i64,
        topic: &Topic,
        store: &PackStore,
    ) -> Result<(), PackPublishError> {
        // TODO indexes: maybe containers(topic_id) and branches(container_id)

        // select segments we'll publish in this topic
        let now = WallTime::now();
        let grace_ms = self.config.pack_sync.peer_publish_grace.as_millis() as u64;
        let peer_grace_due = now.as_millis().saturating_sub(grace_ms);
        let rows = select_cols::<(
            branches::Id,
            segments::Body,
            branches::ContainerId,
            peers::PeerId,
            Nullable<published::PublishedSize>,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(segments::Table)
                .inner_join(
                    branches::Table,
                    Expr::col((segments::Table, segments::BranchId))
                        .eq(Expr::col((branches::Table, branches::Id))),
                )
                // left join because a container with no row is in the default topic
                .left_join(
                    containers::Table,
                    Expr::col((branches::Table, branches::ContainerId))
                        .eq(Expr::col((containers::Table, containers::ContainerId))),
                )
                .inner_join(
                    peers::Table,
                    Expr::col((branches::Table, branches::PeerId))
                        .eq(Expr::col((peers::Table, peers::Id))),
                )
                .left_join(
                    published::Table,
                    Expr::col((branches::Table, branches::Id))
                        .eq(Expr::col((published::Table, published::BranchId)))
                        .and(Expr::col((published::Table, published::RemoteId)).eq(remote_id)),
                )
                .and_where(
                    Func::coalesce([
                        Expr::col((containers::Table, containers::TopicId)),
                        Expr::val(DEFAULT_TOPIC_ID),
                    ])
                    .eq(topic_id),
                )
                .and_where(
                    Expr::col((segments::Table, segments::EndSize)).gt(Func::coalesce([
                        Expr::col((published::Table, published::PublishedSize)),
                        Expr::val(0),
                    ])),
                )
                .and_where(
                    // select our own branches or peer branches that have been pending since before our grace timestamp
                    Expr::col((branches::Table, branches::PeerId))
                        .eq(self.self_db_id)
                        .or(Expr::col((published::Table, published::PendingSince))
                            .lt(peer_grace_due)),
                )
                .order_by_columns([
                    (
                        (branches::Table, branches::Id).into_column_ref(),
                        Order::Asc,
                    ),
                    // order on end_size because its already in the primary key index
                    (
                        (segments::Table, segments::EndSize).into_column_ref(),
                        Order::Asc,
                    ),
                ]),
        )
        .await?;
        let segments = rows.to_vec()?;

        if segments.is_empty() {
            // nothing to write
            return Ok(());
        }

        let write_state = self.get_pack_write_state(remote_id, topic_id).await?;
        let seq = write_state
            .tips
            .iter()
            .max_by_key(|r| r.end_seq)
            .map(|r| r.end_seq)
            .unwrap_or(0);
        let id = PackFileId::new(seq..seq + 1);
        let pack_desc = PackFileDescriptor {
            topic: topic.clone(),
            peer_id: self.self_init.peer_id,
            id: id.clone(),
        };

        let mut pack_builder = PackBuilder::new(pack_desc, write_state.tips.into_iter().collect());
        let mut db_batch = self.db.new_batch();
        let mut seen_peers = HashSet::new();
        // chunk by branches
        for chunk in
            segments.chunk_by(|(a_branch, _, _, _, _), (b_branch, _, _, _, _)| a_branch == b_branch)
        {
            let (branch_id, _, container_id, peer_id, published_size) =
                chunk.first().expect("non-empty chunk");

            // ensure peer inits are written
            let peer_id = PeerId(*peer_id);
            if !seen_peers.contains(&peer_id) {
                seen_peers.insert(peer_id);
            }

            let bodies = chunk
                .iter()
                .map(|(_, body, _, _, _)| body)
                .collect::<Vec<_>>();
            let hash_ctx = LogHashContext::new(&LogId {
                container_id: ContainerId(*container_id),
                peer_id,
            });
            let new_published_size = pack_builder
                .add_container_segments(
                    self.key_resolver.as_ref(),
                    &hash_ctx,
                    published_size.unwrap_or(0),
                    &bodies,
                )
                .await?;
            self.update_published_batch(
                db_batch.as_mut(),
                &IngestSource::Pack { remote_id },
                *branch_id,
                new_published_size,
            )?;
        }

        self.ensure_peer_inits(store, seen_peers).await?;

        let pack_data = pack_builder.build(self.credentials.signing_key())?;
        let entries = super::fs_read::pack_entry_count(&pack_data.header.header);
        let bytes = pack_data.body.len() as u64;

        // write the pack
        store.write_pack(&pack_data).await?;

        // update the db after writing the pack, if we fail updating the db, but the pack was written
        // the write loop will actually update the write state for us
        let mut tips = HashSet::new();
        tips.insert(id.get_ref());
        self.update_pack_write_state_batch(
            db_batch.as_mut(),
            remote_id,
            topic_id,
            PackWriteState { tips },
        )?;
        let mut read_state = self
            .get_pack_read_state(remote_id, topic_id, self.self_db_id)
            .await?;
        let id_for_event = id.clone();
        read_state.consumed.insert(id);
        self.update_pack_read_state_batch(
            db_batch.as_mut(),
            remote_id,
            topic_id,
            self.self_db_id,
            read_state,
        )?;

        db_batch.commit().await?;

        self.sync.update_remote(remote_id, |r| {
            r.last_publish_at = Some(std::time::SystemTime::now())
        });
        self.emit(super::events::SyncEventKind::FilePublished {
            remote: remote_id,
            file: id_for_event,
            entries,
            bytes,
        });

        Ok(())
    }

    pub(crate) async fn update_published(
        &self,
        src: &IngestSource,
        branch_id: i64,
        size: u64,
    ) -> Result<(), DbError> {
        let IngestSource::Pack { remote_id } = src else {
            return Ok(());
        };
        insert_cols::<
            (
                published::RemoteId,
                published::BranchId,
                published::PublishedSize,
                published::PendingSince,
            ),
            (),
        >(
            self.db.as_ref(),
            (*remote_id, branch_id, size, None),
            &mut update_published_stmt(size),
        )
        .await?;
        Ok(())
    }

    pub(crate) fn update_published_batch(
        &self,
        batch: &mut dyn DbBatch,
        src: &IngestSource,
        branch_id: i64,
        size: u64,
    ) -> Result<(), DbError> {
        let IngestSource::Pack { remote_id } = src else {
            return Ok(());
        };
        insert_cols_batch::<(
            published::RemoteId,
            published::BranchId,
            published::PublishedSize,
            published::PendingSince,
        )>(
            batch,
            (*remote_id, branch_id, size, None),
            &mut update_published_stmt(size),
        )
    }

    pub(crate) async fn get_pack_write_state(
        &self,
        remote_id: i64,
        topic_id: i64,
    ) -> Result<PackWriteState, DbError> {
        Ok(select_cols::<(topic_state::WriteState,)>(
            self.db.as_ref(),
            Query::select()
                .from(topic_state::Table)
                .and_where(Expr::col(topic_state::TopicId).eq(topic_id))
                .and_where(Expr::col(topic_state::RemoteId).eq(remote_id)),
        )
        .await?
        .one()?
        .unwrap_or_default()
        .0)
    }

    pub(crate) fn update_pack_write_state_batch(
        &self,
        batch: &mut dyn DbBatch,
        remote_id: i64,
        topic_id: i64,
        new_state: PackWriteState,
    ) -> Result<(), DbError> {
        insert_cols_batch::<(
            topic_state::RemoteId,
            topic_state::TopicId,
            topic_state::WriteState,
            topic_state::Dirty,
        )>(
            batch,
            (remote_id, topic_id, new_state, false),
            Query::insert().into_table(topic_state::Table).on_conflict(
                OnConflict::columns([
                    topic_state::RemoteId.into_iden(),
                    topic_state::TopicId.into_iden(),
                ])
                .update_column(topic_state::WriteState)
                .to_owned(),
            ),
        )
    }

    async fn ensure_peer_inits(
        &self,
        store: &PackStore,
        mut peers: HashSet<PeerId>,
    ) -> Result<(), PackPublishError> {
        let existing = store.list_peer_inits().await?;
        for peer in existing {
            peers.remove(&peer);
        }
        for peer_id in peers {
            match self.resolve_peer(&peer_id).await {
                Ok(Some(info)) => store.write_peer_init(&info.init_entry).await?,
                Ok(None) => tracing::warn!(%peer_id, "can't resolve peer info"),
                Err(error) => tracing::warn!(%peer_id, %error, "can't resolve peer info"),
            }
        }
        Ok(())
    }
}

fn update_published_stmt(size: u64) -> InsertStatement {
    Query::insert()
        .into_table(published::Table)
        .on_conflict(
            OnConflict::columns([
                published::RemoteId.into_iden(),
                published::BranchId.into_iden(),
            ])
            .update_column(published::PublishedSize)
            .update_column(published::PendingSince)
            .action_and_where(Expr::col((published::Table, published::PublishedSize)).lt(size))
            .to_owned(),
        )
        .to_owned()
}
