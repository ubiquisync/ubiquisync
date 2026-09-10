use sea_query::{Expr, ExprTrait, Func, IntoColumnRef, Order, Query};
use ubiquisync_core::{
    ids::{ContainerId, LogId},
    log::{
        ChainHash, ChainSeed,
        segment::{SegmentReader, encode_segment_plaintext},
    },
};
use ubiquisync_fs::pack::SegmentDescriptor;

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

    fn join_segments<'a>(
        &self,
        peer_info: &PeerInfo,
        container_id: &ContainerId,
        bodies: impl Iterator<Item = &'a [u8]>,
    ) -> anyhow::Result<SegmentData> {
        let log_id = LogId {
            peer_id: peer_info.peer,
            container_id: *container_id,
        };
        let seed = ChainSeed::new(&log_id);
        let mut prev_chain = None;
        let mut chain_hash = None;
        let mut all_entries = vec![];
        let mut sig = None;
        for body in bodies {
            let reader = SegmentReader::start(body)?;
            let segment_header = reader.header();
            if let Some(ch) = chain_hash {
                if segment_header.prev_chain != ch {
                    todo!("error")
                }
            } else {
                prev_chain = Some(segment_header.prev_chain);
            }
            let verified =
                reader.verify(&peer_info.commitment.sig_verify_key, &None, &None, &seed)?;
            chain_hash = Some(verified.chain_hash);
            sig = Some(verified.signature);
            let entries = verified.to_plaintext(&None)?;
            all_entries.extend(entries);
        }

        // TODO get rid of unwraps
        let sig = sig.unwrap();
        let prev_chain = prev_chain.unwrap();
        let chain_hash = chain_hash.unwrap();

        let new_body = encode_segment_plaintext(&sig, &prev_chain, &None, &all_entries)?;
        Ok(SegmentData {
            body: new_body,
            prev_chain,
            chain_hash,
        })
    }
}

struct SegmentData {
    prev_chain: ChainHash,
    chain_hash: ChainHash,
    body: Vec<u8>,
}
