use sea_query::{Expr, ExprTrait, Query, value::prelude::Uuid};
use ubiquisync_core::{
    crypto::CipherInfo,
    ids::ContainerId,
    log::{ChainHash, LogHashContext},
};

use crate::{
    db::{
        DbError,
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

impl<R> ReplicaInner<R> {
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
