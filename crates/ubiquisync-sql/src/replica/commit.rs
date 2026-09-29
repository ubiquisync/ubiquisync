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
    reducer::Reducer,
    replica::{
        ReplicaInner,
        ingest::SegmentProcessError,
        schema::{CommitErr, streams},
        streams::StreamInfo,
    },
};

use super::schema::UnknownSoftwareVersion;

#[derive(Debug, Error)]
enum TryCommitError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("commit stall: {0:?}")]
    Stall(CommitErr),
}

impl<R: Reducer> ReplicaInner<R> {
    pub(crate) async fn try_commit(
        &self,
        stream: &StreamInfo,
        segment: &DecodedSegment<'_>,
        hash_ctx: &LogHashContext,
    ) -> Result<(), DbError> {
        match self.do_try_commit(stream, segment, hash_ctx).await {
            Ok(_) => Ok(()),
            Err(TryCommitError::Db(e)) => Err(e),
            Err(TryCommitError::Stall(e)) => self.set_commit_err(stream, e).await,
        }
    }

    async fn do_try_commit(
        &self,
        stream: &StreamInfo,
        segment: &DecodedSegment<'_>,
        hash_ctx: &LogHashContext,
    ) -> Result<(), TryCommitError> {
        if stream.commit_err.is_some() {
            // we actually can't commit now because committing is stalled with an error
            return Ok(());
        }

        let mut commit_size = stream.commit_size;
        if commit_size != stream.head_chain.size {
            todo!("internal error, stream sizes don't match expected based on no err")
        }

        let decoded = match segment.entries {
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
                                Err(crate::op::OpDecodeError::Invalid(e)) => {
                                    return Err(TryCommitError::Stall(CommitErr::Frozen));
                                }
                            };

                            // TODO eventually we want a way for reducers to specify causal dependencies
                            // on other logs as an error, for now all errors cause a stall
                            let read_state = self
                                .reducer
                                .prepare(self.db.as_ref(), &op)
                                .await
                                .map_err(|_| TryCommitError::Stall(CommitErr::Frozen))?;

                            let apply_state = self
                                .reducer
                                .apply(batch.as_mut(), ts, &op, read_state)
                                .map_err(|e| SegmentProcessError::Reducer(Box::new(e)))?;
                            Some(apply_state)
                        }
                        ubiquisync_core::log::EntryBody::UseKey(cipher_info) => {
                            todo!("try to resolve the key!");
                            update_cols_batch::<(streams::CommitSize, streams::CommitErr)>(
                                batch.as_mut(),
                                (
                                    commit_size,
                                    Some(CommitErr::NeedKey(cipher_info.fingerprint)),
                                ),
                                Query::update()
                                    .table(streams::Table)
                                    .and_where(Expr::column(streams::Id).eq(stream.id)),
                            )?;

                            let _ = batch.commit().await?;
                            // we can't resolve encryption keys let so we're done processing
                            // and commit is stalled until encryption support arrives
                            return Ok(());
                        }
                        ubiquisync_core::log::EntryBody::Expunged(_) => {
                            // NOTE: maybe we could skip updating commit index for expunged entries
                            // but for now we don't because an expunge at the end of a segment
                            // could cause an auto-resume proc to loop
                            None
                        }
                    };

                    update_cols_batch::<(streams::CommitSize,)>(
                        batch.as_mut(),
                        (commit_size,),
                        Query::update()
                            .table(streams::Table)
                            .and_where(Expr::column(streams::Id).eq(stream.id)),
                    )?;

                    let batch_result = batch.commit().await?;

                    if let Some(apply_state) = apply_state {
                        self.reducer
                            .post_apply(apply_state, &batch_result)
                            .map_err(|e| SegmentProcessError::Reducer(Box::new(e)))?;
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

    pub(crate) async fn retry_commit(&self) {
        // TODO
        // 1. select any streams where commit_size < head_size AND commit_error == NULL
        // 2. select any streams where commit_error is HLC and clock has advanced
        // 3. other conditions: waiting for peer, key or software upgrade
    }
}
