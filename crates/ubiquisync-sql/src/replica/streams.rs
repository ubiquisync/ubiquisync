use sea_query::{Expr, ExprTrait, Query, value::prelude::Uuid};
use ubiquisync_core::{
    crypto::CipherInfo,
    ids::{ContainerId, LogId},
    log::ChainHash,
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica,
        schema::{CommitErr, HeadErr, streams},
        stream_lock::KeyedLockGuard,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamLog {
    pub peer_db_id: i64,
    pub container_id: ContainerId,
}

impl StreamLog {
    pub(crate) fn new(peer_db_id: i64, container_id: ContainerId) -> Self {
        Self {
            peer_db_id,
            container_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct StreamInfo {
    pub id: i64,
    pub head_chain: ChainHash,
    pub head_cipher: Option<CipherInfo>,
    pub head_err: Option<HeadErr>,
    pub commit_size: u64,
    pub commit_err: Option<CommitErr>,
}

impl<R> Replica<R> {
    pub(crate) async fn resolve_streams(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
    ) -> Result<Vec<StreamInfo>, DbError> {
        let key = guard.key();
        let rows = select_cols::<(
            streams::Id,
            streams::HeadSize,
            streams::HeadHash,
            streams::HeadCipher,
            streams::HeadErr,
            streams::CommitSize,
            streams::CommitErr,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(streams::Table)
                .and_where(Expr::column(streams::PeerId).eq(key.peer_db_id))
                .and_where(
                    Expr::column(streams::ContainerId).eq(Uuid::from_bytes(key.container_id.0)),
                ),
        )
        .await?;
        let mut res = vec![];
        for r in rows.iter() {
            let (id, head_size, head_hash, head_cipher, head_err, commit_size, commit_err) = r?;
            let info = StreamInfo {
                id,
                head_chain: ChainHash {
                    hash: head_hash,
                    size: head_size,
                },
                head_cipher,
                head_err,
                commit_size,
                commit_err,
            };
            res.push(info);
        }
        Ok(res)
    }
}
