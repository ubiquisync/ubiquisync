use std::collections::HashMap;

use sea_query::{Expr, ExprTrait, Func, Order, Query};
use ubiquisync_core::{
    crypto::Hash256,
    ids::{ContainerId, PeerId},
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica,
        fs_sync_schema::{pack_read_state, published},
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

/// One branch as `(author, container, head_size, changed_at)`.
pub type BranchChange = (PeerId, ContainerId, u64, u64);

impl<R> Replica<R> {
    /// Every branch with its local `changed_at` stamp, ordered by stamp.
    pub async fn branch_changes(&self) -> Result<Vec<BranchChange>, DbError> {
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
