use sea_query::{
    Expr, ExprTrait, Func, InsertStatement, IntoColumnRef, IntoIden, OnConflict, Order, Query,
};
use ubiquisync_core::{
    ids::{ContainerId, LogId},
    log::{
        ChainHash,
        segment::{SegmentReader, encode_segment_plaintext},
    },
};

use crate::{
    db::{
        DbBatch, DbError,
        sea_query::{insert_cols, insert_cols_batch, select_cols},
    },
    reducer::Reducer,
    replica::{
        Replica, ReplicaInner,
        fs_sync_schema::{published, topics},
        ingest::IngestSource,
        peers::PeerInfo,
        schema::{containers, segments, streams},
        streams::StreamInfo,
    },
};

impl<R> ReplicaInner<R> {
    // async fn publish_topic(&self, remote_id: i64, topic_id: i64) -> Result<(), ()> {
    //     // TODO if we're selected for other peers, we should give them some grace period to publish their own segments
    //     // TODO indexes: maybe containers(topic_id) and streams(container_id)
    //     let segments = select_cols::<(streams::Id, segments::Body, published::PublishedSize)>(
    //         self.db.as_ref(),
    //         Query::select()
    //             .from(segments::Table)
    //             .inner_join(
    //                 streams::Table,
    //                 Expr::col(segments::StreamId).eq(Expr::col(streams::Id)),
    //             )
    //             .inner_join(
    //                 containers::Table,
    //                 Expr::col(streams::ContainerId).eq(Expr::col(containers::ContainerId)),
    //             )
    //             .left_join(
    //                 published::Table,
    //                 Expr::col(streams::Id).eq(Expr::col(published::StreamId)),
    //             )
    //             .and_where(Expr::col(containers::TopicId).eq(topic_id))
    //             .and_where(Expr::col(published::RemoteId).eq(remote_id))
    //             .and_where(Expr::col(segments::EndSize).gt(Func::coalesce([
    //                 Expr::col(published::PublishedSize),
    //                 Expr::val(0),
    //             ])))
    //             .order_by_columns([
    //                 (streams::Id.into_column_ref(), Order::Asc),
    //                 // order on end_size because its already in the primary key index
    //                 (segments::EndSize.into_column_ref(), Order::Asc),
    //             ]),
    //     )
    //     .await?;

    //     Ok(())
    // }

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
