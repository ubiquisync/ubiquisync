use sea_query::{Expr, ExprTrait, Func, IntoColumnRef, Order, Query};
use ubiquisync_core::{
    ids::{ContainerId, LogId},
    log::{
        ChainHash,
        segment::{SegmentReader, encode_segment_plaintext},
    },
};

use crate::{
    db::sea_query::select_cols,
    reducer::Reducer,
    replica::{
        Replica,
        fs_sync_schema::{published, topics},
        peers::PeerInfo,
        schema::{containers, segments, streams},
    },
};

impl<R: Reducer> Replica<R> {
    async fn publish_topic(&self, remote_id: i64, topic_id: i64) -> anyhow::Result<()> {
        // TODO if we're selected for other peers, we should give them some grace period to publish their own segments
        // TODO indexes: maybe containers(topic_id) and streams(container_id)
        let segments = select_cols::<(streams::Id, segments::Body, published::PublishedSize)>(
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
                .left_join(
                    published::Table,
                    Expr::col(streams::Id).eq(Expr::col(published::StreamId)),
                )
                .and_where(Expr::col(containers::TopicId).eq(topic_id))
                .and_where(Expr::col(published::RemoteId).eq(remote_id))
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
        .await?;

        Ok(())
    }
}
