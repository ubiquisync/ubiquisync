use std::{collections::HashMap, sync::Arc};

use sea_query::{Expr, ExprTrait, Func, OnConflict, Order, Query};
use ubiquisync_core::{
    crypto::Hash256,
    ids::{ContainerId, LogId, PeerId},
    init::InitEntry,
    log::{LogHashContext, segment::SegmentReader},
    pack::SegmentDescriptor,
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols, select_cols},
    },
    reducer::Reducer,
    replica::{
        Replica,
        fs_sync_schema::{pack_read_state, published},
        ingest::{IngestSource, SegmentBytesResolver, SegmentProcessError},
        p2p::P2pHandler,
        schema::{branches, peers},
    },
};

/// Each branch of a log as `(head_size, head_hash, fork_size)`, where `fork_size` is the size
/// a fork branched off its parent at (`None` for the root branch).
pub type BranchHeads = Vec<(u64, Hash256, Option<u64>)>;

/// A replica's sync state, keyed so it can be compared across replicas (local db ids differ
/// between replicas, so logs are keyed by author `PeerId` and `ContainerId`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSnapshot {
    /// This replica's own peer id.
    pub self_peer: PeerId,
    /// Every branch of each log, sorted, as `(head_size, head_hash, fork_size)`.
    pub branches: HashMap<(PeerId, ContainerId), BranchHeads>,
    /// Blocked packs per remote, summed over all topics and publishers.
    pub blocked: HashMap<i64, usize>,
    /// Debug descriptions of every blocked pack, for failure output.
    pub blocked_detail: Vec<String>,
    /// Branches whose head is ahead of what is published on an attached remote,
    /// as `(remote_id, author, container)`, sorted.
    pub unpublished: Vec<(i64, PeerId, ContainerId)>,
}

impl SyncSnapshot {
    /// No blocked packs and nothing left to publish.
    pub fn is_quiescent(&self) -> bool {
        self.blocked.values().all(|n| *n == 0) && self.unpublished.is_empty()
    }
}

impl<R: Send + Sync + 'static> Replica<R> {
    /// The replica's p2p handler. Only here for tests until a transport holds one.
    pub fn p2p_handler(&self) -> Arc<dyn P2pHandler> {
        self.inner.clone()
    }
}

/// One branch as `(author, container, head_size, changed_at)`.
pub type BranchStamp = (PeerId, ContainerId, u64, u64);

impl<R> Replica<R> {
    /// Every branch with its local `changed_at` stamp, ordered by stamp.
    pub async fn branch_changes(&self) -> Result<Vec<BranchStamp>, DbError> {
        let rows = select_cols::<(
            peers::PeerId,
            branches::ContainerId,
            branches::HeadSize,
            branches::ChangedAt,
        )>(
            self.inner.db.as_ref(),
            Query::select()
                .from(branches::Table)
                .inner_join(
                    peers::Table,
                    Expr::col((branches::Table, branches::PeerId))
                        .eq(Expr::col((peers::Table, peers::Id))),
                )
                .order_by((branches::Table, branches::ChangedAt), Order::Asc),
        )
        .await?;
        let mut res = vec![];
        for row in rows.iter() {
            let (peer, container, head_size, changed_at) = row?;
            res.push((PeerId(peer), ContainerId(container), head_size, changed_at));
        }
        Ok(res)
    }

    pub async fn sync_snapshot(&self) -> Result<SyncSnapshot, DbError> {
        let db = self.inner.db.as_ref();

        let rows = select_cols::<(
            peers::PeerId,
            branches::ContainerId,
            branches::HeadSize,
            branches::HeadHash,
            branches::ForkSize,
        )>(
            db,
            Query::select().from(branches::Table).inner_join(
                peers::Table,
                Expr::col((branches::Table, branches::PeerId))
                    .eq(Expr::col((peers::Table, peers::Id))),
            ),
        )
        .await?;
        let mut branch_map: HashMap<(PeerId, ContainerId), BranchHeads> = HashMap::new();
        for row in rows.iter() {
            let (peer, container, head_size, head_hash, fork_size) = row?;
            branch_map
                .entry((PeerId(peer), ContainerId(container)))
                .or_default()
                .push((head_size, head_hash, fork_size));
        }
        for heads in branch_map.values_mut() {
            heads.sort();
        }

        let rows = select_cols::<(pack_read_state::RemoteId, pack_read_state::State)>(
            db,
            Query::select().from(pack_read_state::Table),
        )
        .await?;
        let mut blocked: HashMap<i64, usize> = HashMap::new();
        let mut blocked_detail = vec![];
        for row in rows.iter() {
            let (remote_id, state) = row?;
            *blocked.entry(remote_id).or_default() += state.blocked.len();
            for info in state.blocked.values() {
                blocked_detail.push(format!("remote {remote_id}: {info:?}"));
            }
        }
        blocked_detail.sort();

        let remote_ids: Vec<i64> = self
            .inner
            .pack_remotes
            .lock()
            .await
            .keys()
            .copied()
            .collect();
        let mut unpublished = vec![];
        for remote_id in remote_ids {
            let rows = select_cols::<(peers::PeerId, branches::ContainerId)>(
                db,
                Query::select()
                    .from(branches::Table)
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
                    .and_where(Expr::col((branches::Table, branches::HeadSize)).gt(
                        Func::coalesce([
                            Expr::col((published::Table, published::PublishedSize)),
                            Expr::val(0),
                        ]),
                    )),
            )
            .await?;
            for row in rows.iter() {
                let (peer, container) = row?;
                unpublished.push((remote_id, PeerId(peer), ContainerId(container)));
            }
        }
        unpublished.sort_by_key(|(r, p, c)| (*r, *p, c.0));

        Ok(SyncSnapshot {
            self_peer: self.inner.self_init.peer_id,
            branches: branch_map,
            blocked,
            blocked_detail,
            unpublished,
        })
    }
}

/// What ingest did with a segment body handed to `ingest_p2p_body`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pIngest {
    /// Stored, or nothing new in it.
    Placed,
    /// The body doesn't attach to anything this replica holds of the log.
    NoAnchor,
}

struct BodyResolver<'a>(&'a [u8]);

#[async_trait::async_trait]
impl SegmentBytesResolver for BodyResolver<'_> {
    async fn fetch_segment_bytes<'a>(
        &'a mut self,
    ) -> Result<Option<&'a [u8]>, SegmentProcessError> {
        Ok(Some(self.0))
    }
}

impl<R: Reducer> Replica<R> {
    /// Feeds one segment body of `init`'s log in `container` through ingest, the way a p2p
    /// reader will with what it fetched from a peer. Panics on anything but the two outcomes.
    pub async fn ingest_p2p_body(
        &self,
        init: &InitEntry,
        container: ContainerId,
        body: &[u8],
    ) -> P2pIngest {
        let inner = self.inner.as_ref();
        insert_cols::<(peers::PeerId, peers::CommitmentBytes, peers::Signature), ()>(
            inner.db.as_ref(),
            (
                init.peer_id.0,
                init.commitment_bytes.clone(),
                init.signature,
            ),
            Query::insert()
                .into_table(peers::Table)
                .on_conflict(OnConflict::column(peers::PeerId).do_nothing().to_owned()),
        )
        .await
        .unwrap();
        let peer_info = inner.resolve_peer(&init.peer_id).await.unwrap().unwrap();
        let hash_ctx = LogHashContext::new(&LogId {
            peer_id: init.peer_id,
            container_id: container,
        });
        let segment = SegmentReader::start(body)
            .unwrap()
            .read(inner.key_resolver.as_ref(), &hash_ctx)
            .await
            .unwrap();
        let desc = SegmentDescriptor {
            container_id: container,
            idx_range: segment.header.prev_chain.size..segment.head_chain.size,
            prev_chain_hash: segment.header.prev_chain.hash,
            end_chain_hash: segment.head_chain.hash,
            body_loc: 0..body.len(),
        };
        let res = inner
            .try_ingest_segment(
                &hash_ctx,
                &peer_info,
                &desc,
                &mut BodyResolver(body),
                &IngestSource::P2p,
            )
            .await;
        match res {
            Ok(()) => P2pIngest::Placed,
            Err(SegmentProcessError::Pending) => P2pIngest::NoAnchor,
            Err(e) => panic!("ingest failed: {e}"),
        }
    }
}
