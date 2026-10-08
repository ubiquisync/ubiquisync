use std::collections::HashMap;

use sea_query::{Expr, ExprTrait, Func, Query};
use ubiquisync_core::{
    crypto::Hash256,
    ids::{ContainerId, PeerId},
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica,
        fs_sync_schema::{pack_read_state, published},
        schema::{peers, streams},
    },
};

/// A replica's sync state, keyed so it can be compared across replicas (local db ids differ
/// between replicas, so logs are keyed by author `PeerId` and `ContainerId`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSnapshot {
    /// Every stream of each log, sorted, as `(head_size, head_hash, is_fork)`.
    pub streams: HashMap<(PeerId, ContainerId), Vec<(u64, Hash256, bool)>>,
    /// Blocked packs per remote, summed over all topics and publishers.
    pub blocked: HashMap<i64, usize>,
    /// Streams whose head is ahead of what is published on an attached remote,
    /// as `(remote_id, author, container)`, sorted.
    pub unpublished: Vec<(i64, PeerId, ContainerId)>,
}

impl SyncSnapshot {
    /// No blocked packs and nothing left to publish.
    pub fn is_quiescent(&self) -> bool {
        self.blocked.values().all(|n| *n == 0) && self.unpublished.is_empty()
    }
}

impl<R> Replica<R> {
    pub async fn sync_snapshot(&self) -> Result<SyncSnapshot, DbError> {
        let db = self.inner.db.as_ref();

        let rows = select_cols::<(
            peers::PeerId,
            streams::ContainerId,
            streams::HeadSize,
            streams::HeadHash,
            streams::ParentId,
        )>(
            db,
            Query::select().from(streams::Table).inner_join(
                peers::Table,
                Expr::col(streams::PeerId).eq(Expr::col(peers::Id)),
            ),
        )
        .await?;
        let mut stream_map: HashMap<(PeerId, ContainerId), Vec<(u64, Hash256, bool)>> =
            HashMap::new();
        for row in rows.iter() {
            let (peer, container, head_size, head_hash, parent_id) = row?;
            stream_map
                .entry((PeerId(peer), ContainerId(container)))
                .or_default()
                .push((head_size, head_hash, parent_id.is_some()));
        }
        for heads in stream_map.values_mut() {
            heads.sort();
        }

        let rows = select_cols::<(pack_read_state::RemoteId, pack_read_state::State)>(
            db,
            Query::select().from(pack_read_state::Table),
        )
        .await?;
        let mut blocked: HashMap<i64, usize> = HashMap::new();
        for row in rows.iter() {
            let (remote_id, state) = row?;
            *blocked.entry(remote_id).or_default() += state.blocked.len();
        }

        let remote_ids: Vec<i64> = self.inner.pack_remotes.lock().await.keys().copied().collect();
        let mut unpublished = vec![];
        for remote_id in remote_ids {
            let rows = select_cols::<(peers::PeerId, streams::ContainerId)>(
                db,
                Query::select()
                    .from(streams::Table)
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
                    .and_where(Expr::col(streams::HeadSize).gt(Func::coalesce([
                        Expr::col(published::PublishedSize),
                        Expr::val(0),
                    ]))),
            )
            .await?;
            for row in rows.iter() {
                let (peer, container) = row?;
                unpublished.push((remote_id, PeerId(peer), ContainerId(container)));
            }
        }
        unpublished.sort_by_key(|(r, p, c)| (*r, *p, c.0));

        Ok(SyncSnapshot {
            streams: stream_map,
            blocked,
            unpublished,
        })
    }
}
