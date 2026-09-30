use std::borrow::Borrow;

use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    hlc::{HlcError, wall_ms},
    log::{
        LogEntry, LogHashContext,
        segment::{DecodedEntries, DecodedSegment},
    },
};

use crate::{
    db::{
        DbError,
        sea_query::{update_cols, update_cols_batch},
    },
    reducer::{ApplyError, PrepareError, RebuildScope, Reducer},
    replica::{
        ReplicaInner,
        ingest::SegmentProcessError,
        schema::{CommitErr, streams},
        streams::StreamInfo,
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
    #[error("needs rebuild: {0:?}")]
    NeedsRebuild(RebuildScope),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("commit stall: {0:?}")]
    Stall(CommitErr),
}

impl<R: Reducer> ReplicaInner<R> {
    pub(crate) async fn try_commit(
        &self,
        stream: &StreamInfo,
        segment: &DecodedSegment<'_>,
        hash_ctx: &LogHashContext,
    ) -> Result<(), CommitError> {
        match self.do_try_commit(stream, segment, hash_ctx).await {
            Ok(_) => Ok(()),
            Err(TryCommitError::Stall(e)) => Ok(self.set_commit_err(stream, e).await?),
            Err(TryCommitError::NeedsRebuild(_)) => todo!("rebuild state"),
            Err(TryCommitError::Db(e)) => Err(CommitError::Db(e)),
            Err(TryCommitError::Internal(e)) => Err(CommitError::Internal(e)),
        }
    }

    async fn do_try_commit(
        &self,
        stream: &StreamInfo,
        segment: &DecodedSegment<'_>,
        hash_ctx: &LogHashContext,
    ) -> Result<(), TryCommitError> {
        if stream.commit_err.is_some() {
            // TODO should we do anything here? we may be retrying this stream
            // when the stall condition has been cleared or we hit a retry time
        }

        let mut commit_size = stream.commit_size;
        if commit_size != stream.head_chain.size {
            todo!("internal error, stream sizes don't match expected based on no err")
        }

        let decoded = match &segment.entries {
            DecodedEntries::Opaque(items) => todo!(),
            DecodedEntries::Plaintext(items) => items,
        };

        let local_ts = wall_ms();
        for entry in decoded {
            match entry {
                LogEntry::IndexedEntry(entry) => {
                    commit_size += 1;
                    let mut batch = self.db.new_batch();
                    let apply_state = match entry {
                        ubiquisync_core::log::EntryBody::Op(op_entry) => {
                            let ts = op_entry.timestamp;
                            match self.hlc.observe(ts, local_ts, batch.as_mut()) {
                                Ok(_) => {}
                                Err(HlcError::Skew(e)) => {
                                    return Err(TryCommitError::Stall(CommitErr::HLCForwardSkew(
                                        e.received,
                                    )));
                                }
                                Err(HlcError::Storage(e)) => {
                                    return Err(e.into());
                                }
                            }

                            let op = match self
                                .reducer
                                .codec()
                                .decode(&hash_ctx.log_id().container_id, op_entry.op.borrow())
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
                                Err(PrepareError::NeedsRebuild(e)) => {
                                    return Err(TryCommitError::NeedsRebuild(e));
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
                        ubiquisync_core::log::EntryBody::UseKey(cipher_info) => {
                            todo!("try to resolve the key!");
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
                        (commit_size, None),
                        Query::update()
                            .table(streams::Table)
                            .and_where(Expr::column(streams::Id).eq(stream.id)),
                    )?;

                    let batch_result = batch.commit().await?;

                    if let Some(apply_state) = apply_state {
                        self.reducer.post_apply(apply_state, &batch_result)
                    }
                }
                LogEntry::Signature(_) => {} // do nothing
            }
        }
        Ok(())
    }

    async fn set_commit_err(&self, stream: &StreamInfo, err: CommitErr) -> Result<(), DbError> {
        update_cols::<(streams::CommitErr,)>(
            self.db.as_ref(),
            (Some(err),),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream.id)),
        )
        .await?;
        Ok(())
    }

    async fn set_internal_commit_err(&self, stream: &StreamInfo) -> Result<(), DbError> {
        let ts = wall_ms();
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
        // TODO
        // 1. select any streams where commit_size < head_size AND commit_error == NULL
        // 2. select any streams where commit_error is HLC and clock has advanced
        // 3. other conditions: waiting for peer, key or software upgrade
    }
}
