use std::sync::atomic::Ordering;

use sea_query::{Expr, ExprTrait, Order, Query, value::prelude::Uuid};
use ubiquisync_core::{
    crypto::CipherInfo,
    hlc::WallTime,
    ids::ContainerId,
    log::{ChainHash, LogHashContext},
};

use crate::{
    db::{
        Db, DbError,
        sea_query::{insert_cols, select_cols},
    },
    replica::{
        ReplicaInner,
        keyed_lock::KeyedLockGuard,
        schema::{CommitErr, HeadErr, branches},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LogKey {
    pub peer_db_id: i64,
    pub container_id: ContainerId,
}

impl LogKey {
    pub(crate) fn new(peer_db_id: i64, container_id: ContainerId) -> Self {
        Self {
            peer_db_id,
            container_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchInfo {
    pub id: i64,
    pub head_chain: ChainHash,
    pub head_cipher: Option<CipherInfo>,
    pub head_err: Option<HeadErr>,
    pub commit_size: u64,
    pub commit_err: Option<CommitErr>,
    pub parent_id: Option<i64>,
    pub fork_size: Option<u64>,
}

/// The highest `changed_at` stamp saved in the db, 0 when there are no branches.
pub(crate) async fn load_last_changed_at(db: &dyn Db) -> Result<u64, DbError> {
    Ok(select_cols::<(branches::ChangedAt,)>(
        db,
        Query::select()
            .from(branches::Table)
            .order_by(branches::ChangedAt, Order::Desc)
            .limit(1),
    )
    .await?
    .one()?
    .map(|(changed_at,)| changed_at)
    .unwrap_or(0))
}

/// The stamp after `last`: the wall clock, or `last + 1` when the wall clock isn't past it.
fn changed_at_after(last: u64, wall_ms: u64) -> u64 {
    std::cmp::max(wall_ms, last.saturating_add(1))
}

impl<R> ReplicaInner<R> {
    /// The stamp to save in `branches.changed_at` whenever a branch is created or its head
    /// moves. Stamps are wall clock milliseconds, bumped so they are unique and always
    /// increase on this replica, also across restarts (the last one is loaded from the db).
    ///
    /// Wall clock time and not a counter, so that a db restored from a backup doesn't hand
    /// out stamps it already used before the backup was restored: a peer that remembers how
    /// far it read sees whatever changes after the restore as new.
    ///
    /// The stamp is taken before its batch commits, so two logs written at the same time can
    /// commit in the opposite order to their stamps. A reader paging by stamp has to allow
    /// for that (re-read a little before its last stamp).
    pub(crate) fn next_changed_at(&self) -> u64 {
        let wall_ms = WallTime::now().as_millis();
        let mut last = self.last_changed_at.load(Ordering::Relaxed);
        loop {
            let next = changed_at_after(last, wall_ms);
            match self.last_changed_at.compare_exchange_weak(
                last,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return next,
                Err(cur) => last = cur,
            }
        }
    }

    pub(crate) async fn resolve_branches(
        &self,
        guard: &KeyedLockGuard<LogKey>,
    ) -> Result<Vec<BranchInfo>, DbError> {
        let key = guard.key();
        let rows = select_cols::<(
            branches::Id,
            branches::HeadSize,
            branches::HeadHash,
            branches::HeadCipher,
            branches::HeadErr,
            branches::CommitSize,
            branches::CommitErr,
            branches::ParentId,
            branches::ForkSize,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(branches::Table)
                .and_where(Expr::column(branches::PeerId).eq(key.peer_db_id))
                .and_where(
                    Expr::column(branches::ContainerId).eq(Uuid::from_bytes(key.container_id.0)),
                ),
        )
        .await?;
        let mut res = vec![];
        for r in rows.iter() {
            let (
                id,
                head_size,
                head_hash,
                head_cipher,
                head_err,
                commit_size,
                commit_err,
                parent_id,
                fork_size,
            ) = r?;
            let info = BranchInfo {
                id,
                head_chain: ChainHash {
                    hash: head_hash,
                    size: head_size,
                },
                head_cipher,
                head_err,
                commit_size,
                commit_err,
                parent_id,
                fork_size,
            };
            res.push(info);
        }
        Ok(res)
    }

    pub(crate) async fn create_branch(
        &self,
        guard: &KeyedLockGuard<LogKey>,
        hash_ctx: &LogHashContext,
    ) -> Result<BranchInfo, DbError> {
        let head_chain = ChainHash::empty(hash_ctx);
        let (id,) = insert_cols::<
            (
                branches::PeerId,
                branches::ContainerId,
                branches::HeadSize,
                branches::HeadHash,
                branches::CommitSize,
                branches::ChangedAt,
            ),
            (branches::Id,),
        >(
            self.db.as_ref(),
            (
                guard.key().peer_db_id,
                hash_ctx.log_id().container_id.0,
                0,
                head_chain.hash,
                0,
                self.next_changed_at(),
            ),
            Query::insert().into_table(branches::Table),
        )
        .await?
        .exactly_one()?;
        Ok(BranchInfo {
            id,
            head_chain,
            head_cipher: None,
            head_err: None,
            commit_size: 0,
            commit_err: None,
            fork_size: None,
            parent_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::changed_at_after;

    #[test_case(0, 1_000 => 1_000 ; "first stamp is the wall clock")]
    #[test_case(900, 1_000 => 1_000 ; "wall clock ahead of the last stamp")]
    #[test_case(1_000, 1_000 => 1_001 ; "same millisecond")]
    #[test_case(5_000, 1_000 => 5_001 ; "wall clock behind the last stamp")]
    #[test_case(u64::MAX, 1_000 => u64::MAX ; "saturates")]
    fn next_stamp(last: u64, wall_ms: u64) -> u64 {
        changed_at_after(last, wall_ms)
    }
}
