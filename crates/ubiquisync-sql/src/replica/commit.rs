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
        branches::{BranchInfo, LogKey},
        keyed_lock::KeyedLockGuard,
        schema::{CommitErr, branches},
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
    #[tracing::instrument(skip_all)]
    pub(crate) async fn try_commit(
        &self,
        guard: &KeyedLockGuard<LogKey>,
        branch: &mut BranchInfo,
        segment: DecodedSegment<'_>,
    ) -> Result<(), CommitError> {
        match self.do_try_commit(guard, branch, segment).await {
            Ok(_) => Ok(()),
            Err(TryCommitError::Stall(e)) => Ok(self.set_commit_err(branch, e).await?),
            Err(TryCommitError::NeedsRebuild { scope, reason }) => {
                // TODO rebuild state, until then remote data must not panic the sync task
                tracing::warn!(
                    branch_id = branch.id,
                    ?scope,
                    reason,
                    "commit needs rebuild, stalling"
                );
                Ok(self.set_internal_commit_err(branch).await?)
            }
            Err(TryCommitError::Db(e)) => {
                // record the stall so the retry loop backs off (if the db is unreachable this
                // write fails too, and the branch is simply tried again on the next tick)
                if let Err(error) = self.set_internal_commit_err(branch).await {
                    tracing::warn!(branch_id = branch.id, %error, "couldn't record commit stall");
                }
                Err(CommitError::Db(e))
            }
            Err(TryCommitError::Internal(e)) => Err(CommitError::Internal(e)),
        }
    }

    async fn do_try_commit(
        &self,
        _guard: &KeyedLockGuard<LogKey>,
        branch: &mut BranchInfo,
        segment: DecodedSegment<'_>,
    ) -> Result<(), TryCommitError> {
        if let Some(CommitErr::Frozen) = branch.commit_err {
            // branch is frozen, nothing to do
            return Ok(());
        }

        let log_id = *segment.chain_seed.log_id();
        for res in segment
            .to_plaintext(self.key_resolver.as_ref())
            .await
            .entries
        {
            let (entry, meta) = match res {
                Ok((entry, meta)) => (entry, meta),
                Err(e) => match e {
                    SegmentCipherError::CipherError(_) => {
                        return Ok(self.set_internal_commit_err(branch).await?);
                    }
                    SegmentCipherError::ChainHashError(e) => match e {
                        // if we overflow we must freeze the chain, but this literally can never occur
                        ChainHashError::SizeOverflow => {
                            return Ok(self.set_commit_err(branch, CommitErr::Frozen).await?);
                        }
                    },
                    SegmentCipherError::KeyResolve(e) => match e {
                        CipherKeyResolveError::NotFound(k) => {
                            return Ok(self.set_commit_err(branch, CommitErr::NeedKey(k)).await?);
                        }
                        CipherKeyResolveError::UnknownSuite(s) => {
                            return Ok(self
                                .set_commit_err(
                                    branch,
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
                        return Ok(self.set_commit_err(branch, CommitErr::Frozen).await?);
                    }
                },
            };

            if meta.chain_hash.size <= branch.commit_size {
                // we've already committed this entry
                continue;
            } else if meta.chain_hash.size != branch.commit_size + 1 {
                return Err(TryCommitError::Internal(
                    "caller error: segment starts after commit size".into(),
                ));
            }

            // TODO add UPDATE clause to clear branch deps waiting on this entry

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
                                    return Ok(self.set_internal_commit_err(branch).await?);
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
                                                .set_internal_commit_err(branch)
                                                .await?);
                                        }
                                    },
                                ),
                                Err(PrepareError::Db(_)) | Err(PrepareError::Internal(_)) => {
                                    return Ok(self.set_internal_commit_err(branch).await?);
                                }
                                Err(PrepareError::NeedsRebuild { scope, reason }) => {
                                    return Err(TryCommitError::NeedsRebuild { scope, reason });
                                }
                                Err(PrepareError::NeedsDeps(_)) => {
                                    // TODO insert branch_deps rows, until then the branch stalls
                                    return Err(TryCommitError::Stall(CommitErr::AwaitingDeps));
                                }
                                // we skip this op and don't call post-apply
                                Err(PrepareError::InvalidOp(_)) => None,
                                Err(PrepareError::Frozen(_)) => {
                                    return Ok(self
                                        .set_commit_err(branch, CommitErr::Frozen)
                                        .await?);
                                }
                            }
                        }
                        ubiquisync_core::log::EntryBody::UseKey(_) => {
                            // we just advance commit here, no special processing needed because
                            // we don't store commit ciphers in the branches table
                            None
                        }
                        ubiquisync_core::log::EntryBody::Expunged(_) => {
                            // NOTE: maybe we could skip updating commit index for expunged entries
                            // but for now we don't because an expunge at the end of a segment
                            // could cause an auto-resume proc to loop
                            None
                        }
                    };

                    update_cols_batch::<(branches::CommitSize, branches::CommitErr)>(
                        batch.as_mut(),
                        (branch.commit_size + 1, None),
                        Query::update()
                            .table(branches::Table)
                            .and_where(Expr::column(branches::Id).eq(branch.id)),
                    )?;

                    let batch_result = batch.commit().await?;

                    branch.commit_size += 1;
                    branch.commit_err = None;

                    if let Some(apply_state) = apply_state {
                        self.reducer.post_apply(apply_state, &batch_result)
                    }
                }
                LogEntry::Signature(_) => {} // do nothing
            }
        }
        Ok(())
    }

    async fn set_commit_err(&self, branch: &mut BranchInfo, err: CommitErr) -> Result<(), DbError> {
        update_cols::<(branches::CommitErr,)>(
            self.db.as_ref(),
            (Some(err.clone()),),
            Query::update()
                .table(branches::Table)
                .and_where(Expr::column(branches::Id).eq(branch.id)),
        )
        .await?;
        branch.commit_err = Some(err);
        Ok(())
    }

    pub(crate) async fn set_internal_commit_err(
        &self,
        branch: &mut BranchInfo,
    ) -> Result<(), DbError> {
        let err = next_internal_err(branch.commit_err.as_ref(), WallTime::now());
        self.set_commit_err(branch, err).await
    }
}

/// The internal stall to record for a failure at `now`: a repeat failure keeps when it first
/// failed and counts one more retry, anything else starts over.
fn next_internal_err(prev: Option<&CommitErr>, now: WallTime) -> CommitErr {
    let now = now.as_millis();
    match prev {
        Some(CommitErr::Internal {
            retry_time_span,
            retry_count,
        }) => CommitErr::Internal {
            // never let the span end before it starts if the wall clock stepped backwards
            retry_time_span: retry_time_span.start..Ord::max(now, retry_time_span.start),
            retry_count: retry_count + 1,
        },
        _ => CommitErr::Internal {
            retry_time_span: now..now,
            retry_count: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    fn internal(first_failed: u64, last_failed: u64, retry_count: u64) -> CommitErr {
        CommitErr::Internal {
            retry_time_span: first_failed..last_failed,
            retry_count,
        }
    }

    #[test_case(None, 50 => internal(50, 50, 0) ; "first failure")]
    #[test_case(Some(CommitErr::AwaitingDeps), 50 => internal(50, 50, 0) ; "after another stall")]
    #[test_case(Some(internal(10, 20, 3)), 50 => internal(10, 50, 4) ; "repeat failure")]
    #[test_case(Some(internal(10, 20, 3)), 5 => internal(10, 10, 4) ; "clock stepped back")]
    fn internal_err(prev: Option<CommitErr>, now: u64) -> CommitErr {
        next_internal_err(prev.as_ref(), WallTime::from_millis(now))
    }
}
