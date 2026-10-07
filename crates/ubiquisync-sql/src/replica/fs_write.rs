use std::collections::{HashMap, HashSet};

use crate::{
    db::{
        DbBatch, DbError,
        sea_query::{insert_cols, insert_cols_batch, select_cols},
    },
    reducer::Reducer,
    replica::{
        ReplicaInner,
        fs_sync_schema::{PackWriteState, published, topic_state},
        ingest::IngestSource,
        schema::{containers, peers, segments, streams},
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
    pack::{PackBuilder, PackFileDescriptor, PackFileId, PackStore, Topic},
};

#[derive(Error, Debug)]
pub(crate) enum PackPublishError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
}

impl<R: Reducer> ReplicaInner<R> {
    async fn publish_topic(
        &self,
        remote_id: i64,
        topic_id: i64,
        topic: &Topic,
        store: &PackStore,
    ) -> Result<(), PackPublishError> {
        // TODO indexes: maybe containers(topic_id) and streams(container_id)

        // TODO: timestamp peer streams which we hold but haven't seen published on this remote as pending
        // so we know to publish them ourselves if the grace period expires
        // let now = WallTime::now();
        // insert_cols::<(
        //     published::RemoteId,
        //     published::StreamId,
        //     published::PublishedSize,
        //     published::PendingSince,
        // )>(self.db.as_ref(), (), Query::insert())
        // .await?;

        // select segments we'll publish in this topic
        let segments = select_cols::<(
            streams::Id,
            segments::Body,
            streams::ContainerId,
            peers::PeerId,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(segments::Table)
                .inner_join(
                    streams::Table,
                    Expr::col(segments::StreamId).eq(Expr::col(streams::Id)),
                )
                .inner_join(
                    containers::Table,
                    Expr::col(streams::ContainerId).eq(Expr::col(containers::ContainerId)),
                )
                .inner_join(
                    peers::Table,
                    Expr::col(streams::PeerId).eq(Expr::col(peers::Id)),
                )
                .left_join(
                    published::Table,
                    Expr::col(streams::Id)
                        .eq(Expr::col(published::StreamId))
                        .and(Expr::col(published::RemoteId).eq(remote_id)),
                )
                .and_where(Expr::col(containers::TopicId).eq(topic_id))
                .and_where(Expr::col(segments::EndSize).gt(Func::coalesce([
                    Expr::col(published::PublishedSize),
                    Expr::val(0),
                ])))
                .order_by_columns([
                    (streams::Id.into_column_ref(), Order::Asc),
                    // order on end_size because its already in the primary key index
                    (segments::EndSize.into_column_ref(), Order::Asc),
                ]),
        )
        .await?
        .to_vec()?;

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
            peer_id: self.self_id,
            id,
        };

        let mut pack_builder = PackBuilder::new(pack_desc, write_state.tips.into_iter().collect());
        let mut sql_batch = self.db.new_batch();
        // chunk by streams
        for chunk in
            segments.chunk_by(|(a_stream, _, _, _), (b_stream, _, _, _)| a_stream == b_stream)
        {
            let (stream_id, _, container_id, peer_id) = chunk.first().expect("non-empty chunk");
            let bodies = chunk.iter().map(|(_, body, _, _)| body).collect::<Vec<_>>();
            let hash_ctx = LogHashContext::new(&LogId {
                container_id: ContainerId(*container_id),
                peer_id: PeerId(*peer_id),
            });
            let published_size = pack_builder
                .add_container_segments(self.key_resolver.as_ref(), &hash_ctx, &bodies)
                .await?;
            self.update_published_batch(
                &mut sql_batch,
                &IngestSource::Pack { remote_id },
                stream_id,
                published_size,
            )?;
        }

        let pack_data = pack_builder.build(self.credentials.signing_key())?;

        store.write_pack(&pack_data).await?;

        let mut tips = HashSet::new();
        tips.insert(id.get_ref());
        sql_batch.commit().await?;
        // TODO update pack write state in batch
        self.update_pack_write_state(remote_id, topic_id, PackWriteState { tips })
            .await?;
        // TODO add pack to read state consumed

        Ok(())
    }

    pub(crate) async fn update_published(
        &self,
        src: &IngestSource,
        stream_id: i64,
        size: u64,
    ) -> Result<(), DbError> {
        let IngestSource::Pack { remote_id } = src else {
            return Ok(());
        };
        insert_cols::<
            (
                published::RemoteId,
                published::StreamId,
                published::PublishedSize,
            ),
            (),
        >(
            self.db.as_ref(),
            (*remote_id, stream_id, size),
            &mut update_published_stmt(size),
        )
        .await?;
        Ok(())
    }

    pub(crate) fn update_published_batch(
        &self,
        batch: &mut dyn DbBatch,
        src: &IngestSource,
        stream_id: i64,
        size: u64,
    ) -> Result<(), DbError> {
        let IngestSource::Pack { remote_id } = src else {
            return Ok(());
        };
        insert_cols_batch::<(
            published::RemoteId,
            published::StreamId,
            published::PublishedSize,
        )>(
            batch,
            (*remote_id, stream_id, size),
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

    pub(crate) async fn update_pack_write_state(
        &self,
        remote_id: i64,
        topic_id: i64,
        new_state: PackWriteState,
    ) -> Result<(), DbError> {
        insert_cols::<
            (
                topic_state::RemoteId,
                topic_state::TopicId,
                topic_state::WriteState,
            ),
            (),
        >(
            self.db.as_ref(),
            (remote_id, topic_id, new_state),
            Query::insert().into_table(topic_state::Table).on_conflict(
                OnConflict::columns([
                    topic_state::RemoteId.into_iden(),
                    topic_state::TopicId.into_iden(),
                ])
                .update_column(topic_state::WriteState)
                .to_owned(),
            ),
        )
        .await?;
        Ok(())
    }
}

fn update_published_stmt(size: u64) -> InsertStatement {
    Query::insert()
        .into_table(published::Table)
        .on_conflict(
            OnConflict::columns([
                published::RemoteId.into_iden(),
                published::StreamId.into_iden(),
            ])
            .update_column(published::PublishedSize)
            .action_and_where(Expr::col((published::Table, published::PublishedSize)).lt(size))
            .to_owned(),
        )
        .to_owned()
}
