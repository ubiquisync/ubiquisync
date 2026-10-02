use std::borrow::Borrow;

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::CipherKeyResolveError,
    hlc::WallTime,
    log::{ChainHashError, LogEntry, SegmentCipherError, segment::DecodedSegment},
};

use crate::{
    db::{
        DbError,
        sea_query::{update_cols, update_cols_batch},
    },
    reducer::{PrepareError, RebuildScope, Reducer},
    replica::{
        HlcError, ReplicaInner,
        schema::{CommitErr, streams},
        stream_lock::KeyedLockGuard,
        streams::{StreamInfo, StreamLog},
    },
};

use super::schema::UnknownSoftwareVersion;

#[derive(Debug, Error)]
pub enum CommitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Error)]
enum TryCommitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("needs rebuild, reason: {reason}, scope: {scope:?}")]
    NeedsRebuild { scope: RebuildScope, reason: String },
    #[error("internal error: {0}")]
    Internal(String),
    #[error("commit stall: {0:?}")]
    Stall(CommitErr),
}

impl<R: Reducer> ReplicaInner<R> {
    async fn try_commit(
        &self,
        guard: &KeyedLockGuard<StreamLog>,
        stream: &mut StreamInfo,
        segment: DecodedSegment<'_>,
    ) -> Result<(), CommitError> {
        match self.do_try_commit(guard, stream, segment).await {
            Ok(_) => Ok(()),
            Err(TryCommitError::Stall(e)) => Ok(self.set_commit_err(stream, e).await?),
            Err(TryCommitError::NeedsRebuild { .. }) => todo!("rebuild state"),
            Err(TryCommitError::Db(e)) => Err(CommitError::Db(e)),
            Err(TryCommitError::Internal(e)) => Err(CommitError::Internal(e)),
        }
    }

    async fn do_try_commit(
        &self,
        _guard: &KeyedLockGuard<StreamLog>,
        stream: &mut StreamInfo,
        segment: DecodedSegment<'_>,
    ) -> Result<(), TryCommitError> {
        if let Some(CommitErr::Frozen) = stream.commit_err {
            // stream is frozen, nothing to do
            return Ok(());
        }

        let log_id = *segment.chain_seed.log_id();
        for res in segment
            .to_plaintext(self.key_resolver.as_ref())
            .await
            .entries
        {
            let (entry, hash) = match res {
                Ok((entry, hash)) => (entry, hash),
                Err(e) => match e {
                    SegmentCipherError::CipherError(_) => {
                        return Ok(self.set_internal_commit_err(stream).await?);
                    }
                    SegmentCipherError::ChainHashError(e) => match e {
                        // if we overflow we must freeze the chain, but this literally can never occur
                        ChainHashError::SizeOverflow => {
                            return Ok(self.set_commit_err(stream, CommitErr::Frozen).await?);
                        }
                    },
                    SegmentCipherError::KeyResolve(e) => match e {
                        CipherKeyResolveError::NotFound(k) => {
                            return Ok(self.set_commit_err(stream, CommitErr::NeedKey(k)).await?);
                        }
                        CipherKeyResolveError::UnknownSuite(s) => {
                            return Ok(self
                                .set_commit_err(
                                    stream,
                                    CommitErr::IncompatibleSoftware(
                                        UnknownSoftwareVersion::CipherSuite(s),
                                    ),
                                )
                                .await?);
                        }
                    },
                    SegmentCipherError::InvalidTimestamp | SegmentCipherError::Validation(_) => {
                        // we could decide to just skip these entries, but that would require
                        // a complex refactoring of to_plaintext to continue on such errors
                        // and it should literally be impossible to construct an entry that hits
                        // this case if you're building entries correctly
                        return Ok(self.set_commit_err(stream, CommitErr::Frozen).await?);
                    }
                },
            };

            if hash.size <= stream.commit_size {
                // we've already committed this entry
                continue;
            } else if hash.size != stream.commit_size + 1 {
                return Err(TryCommitError::Internal(
                    "caller error: segment starts after commit size".into(),
                ));
            }

            // TODO add UPDATE clause to clear stream deps waiting on this entry

            match entry {
                LogEntry::IndexedEntry(entry) => {
                    let mut batch = self.db.new_batch();
                    let apply_state = match entry {
                        ubiquisync_core::log::EntryBody::Op(op_entry) => {
                            let ts = op_entry.timestamp;
                            match self.observe_remote_hlc(batch.as_mut(), ts) {
                                Ok(_) => {}
                                Err(HlcError::Skew { remote, .. }) => {
                                    return Err(TryCommitError::Stall(CommitErr::HLCForwardSkew(
                                        remote,
                                    )));
                                }
                                Err(_) => {
                                    return Ok(self.set_internal_commit_err(stream).await?);
                                }
                            }

                            let op = match self
                                .reducer
                                .codec()
                                .decode(&log_id.container_id, op_entry.op.borrow())
                            {
                                Ok(op) => op,
                                // decode errors are either:
                                // - incompatible software version
                                // - frozen stall
                                Err(crate::op::OpDecodeError::UnknownOpTag(t)) => {
                                    return Err(TryCommitError::Stall(
                                        CommitErr::IncompatibleSoftware(
                                            UnknownSoftwareVersion::OpType(t),
                                        ),
                                    ));
                                }
                                Err(crate::op::OpDecodeError::Invalid(_)) => {
                                    return Err(TryCommitError::Stall(CommitErr::Frozen));
                                }
                            };

                            match self.reducer.prepare(self.db.as_ref(), &op).await {
                                Ok(read_state) => Some(
                                    match self.reducer.apply(batch.as_mut(), ts, &op, read_state) {
                                        Ok(apply_state) => apply_state,
                                        _ => {
                                            return Ok(self
                                                .set_internal_commit_err(stream)
                                                .await?);
                                        }
                                    },
                                ),
                                Err(PrepareError::Db(_)) | Err(PrepareError::Internal(_)) => {
                                    return Ok(self.set_internal_commit_err(stream).await?);
                                }
                                Err(PrepareError::NeedsRebuild { scope, reason }) => {
                                    return Err(TryCommitError::NeedsRebuild { scope, reason });
                                }
                                Err(PrepareError::NeedsDeps(_)) => {
                                    todo!("insert dep rows")
                                }
                                // we skip this op and don't call post-apply
                                Err(PrepareError::InvalidOp(_)) => None,
                                Err(PrepareError::Frozen(_)) => {
                                    return Ok(self
                                        .set_commit_err(stream, CommitErr::Frozen)
                                        .await?);
                                }
                            }
                        }
                        ubiquisync_core::log::EntryBody::UseKey(_) => {
                            // we just advance commit here, no special processing needed because
                            // we don't store commit ciphers in the streams table
                            None
                        }
                        ubiquisync_core::log::EntryBody::Expunged(_) => {
                            // NOTE: maybe we could skip updating commit index for expunged entries
                            // but for now we don't because an expunge at the end of a segment
                            // could cause an auto-resume proc to loop
                            None
                        }
                    };

                    update_cols_batch::<(streams::CommitSize, streams::CommitErr)>(
                        batch.as_mut(),
                        (stream.commit_size + 1, None),
                        Query::update()
                            .table(streams::Table)
                            .and_where(Expr::column(streams::Id).eq(stream.id)),
                    )?;

                    let batch_result = batch.commit().await?;

                    stream.commit_size += 1;
                    stream.commit_err = None;

                    if let Some(apply_state) = apply_state {
                        self.reducer.post_apply(apply_state, &batch_result)
                    }
                }
                LogEntry::Signature(_) => {} // do nothing
            }
        }
        Ok(())
    }

    async fn set_commit_err(&self, stream: &mut StreamInfo, err: CommitErr) -> Result<(), DbError> {
        update_cols::<(streams::CommitErr,)>(
            self.db.as_ref(),
            (Some(err.clone()),),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream.id)),
        )
        .await?;
        stream.commit_err = Some(err);
        Ok(())
    }

    async fn set_internal_commit_err(&self, stream: &mut StreamInfo) -> Result<(), DbError> {
        let ts = WallTime::now().as_millis();
        let err = if let Some(CommitErr::Internal {
            retry_time_span,
            retry_count,
        }) = &stream.commit_err
        {
            CommitErr::Internal {
                retry_time_span: retry_time_span.start..ts,
                retry_count: retry_count + 1,
            }
        } else {
            CommitErr::Internal {
                retry_time_span: ts..ts,
                retry_count: 0,
            }
        };
        self.set_commit_err(stream, err).await
    }

    pub(crate) async fn retry_commit(&self) {
        // TODO setup tokio task interval loop
        // TODO
        // 1. select any streams where commit_size < head_size AND commit_error == NULL
        // 2. select any streams where commit_error is HLC and clock has advanced
        // 3. other conditions: waiting for peer, key or software upgrade
    }
}
