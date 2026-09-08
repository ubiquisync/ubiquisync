use std::time::Duration;

use backon::{ConstantBuilder, Retryable};
use sea_query::{Expr, ExprTrait, Query};
use ubiquisync_core::{
    ids::LogId,
    log::{
        ChainHash, ChainSeed, EntryBody, OpBatch, PlaintextLogEntry,
        segment::encode_segment_plaintext,
    },
    uuid::Uuid,
};

use crate::{
    Exec, ExecError,
    db::{
        DbError,
        sea_query::{insert_cols, insert_cols_batch, select_cols, update_cols_batch},
    },
    reducer::Reducer,
    replica::{
        Replica,
        schema::{CommitStatus, segments, streams},
    },
};

#[async_trait::async_trait]
impl<R: Reducer> Exec<R::Op> for Replica<R> {
    /// Apply a local write, minting a fresh log entry for it.
    #[tracing::instrument(skip_all)]
    async fn exec(&self, server_user_id: Option<Uuid>, op: R::Op) -> Result<(), ExecError> {
        (|| self.do_exec(server_user_id, &op))
            .retry(
                ConstantBuilder::new()
                    .with_delay(Duration::from_millis(10))
                    .with_jitter()
                    .with_max_times(5),
            )
            .when(|e| match e {
                // TODO check other errors, maybe extract into a helper if used elsewhere
                ExecError::Db(DbError::UniqueViolation) => true,
                _ => false,
            })
            .notify(|err, _| tracing::debug!(%err, "retrying"))
            .await
    }
}

impl<R: Reducer> Replica<R> {
    async fn do_exec(&self, server_user_id: Option<Uuid>, op: &R::Op) -> Result<(), ExecError> {
        let (container_id, op_bytes) = self.reducer.codec().encode(op)?;
        let log_id = LogId {
            peer_id: self.self_id,
            container_id,
        };
        // per-stream mutex guard to prevent ensures only one thread touches a single stream
        let _guard = self.stream_locks.lock(&log_id).await;

        let stream_rows = select_cols::<(
            streams::Id,
            streams::HeadSize,
            streams::HeadHash,
            streams::HeadCipher,
            streams::HeadStatus,
            streams::CommitSize,
            streams::CommitStatus,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(streams::Table)
                .and_where(Expr::column(streams::PeerId).eq(self.self_db_id))
                .and_where(Expr::column(streams::ContainerId).eq(container_id.as_ref())),
        )
        .await
        .map_err(ExecError::Db)?;

        let seed = ChainSeed::new(&log_id);

        let (stream_id, chain_head, commit_status) = if stream_rows.is_empty() {
            let res = insert_cols::<
                (
                    streams::PeerId,
                    streams::ContainerId,
                    streams::HeadSize,
                    streams::HeadHash,
                    streams::CommitSize,
                    streams::CommitStatus,
                ),
                (streams::Id,),
            >(
                self.db.as_ref(),
                (
                    self.self_db_id,
                    container_id.0,
                    0,
                    *seed.hash(),
                    0,
                    CommitStatus::Ok,
                ),
                Query::insert().into_table(streams::Table),
            )
            .await?;
            let (stream_id,) = res.exactly_one()?;
            (stream_id, ChainHash::empty(&seed), CommitStatus::Ok)
        } else if stream_rows.len() > 1 {
            todo!("found multiple rows, this means we have a fork and need to know what to do")
        } else {
            let (
                stream_id,
                head_size,
                head_hash,
                head_cipher,
                head_status,
                commit_size,
                commit_status,
            ) = stream_rows.exactly_one()?;

            if head_status.is_some() {
                todo!("handle some unexpected status")
            }

            if head_cipher.is_some() {
                todo!("cipher not supported yet");
            }

            if commit_status == CommitStatus::Ok && commit_size != head_size {
                return Err(ExecError::Internal(format!(
                    "commit status is okay but head {head_size} and commit {commit_size} sizes do not match"
                )));
            }

            let chain_head = ChainHash {
                hash: head_hash,
                size: head_size,
            };

            (stream_id, chain_head, commit_status)
        };

        let mut batch = self.db.new_batch();
        let timestamp = self.hlc.now(batch.as_mut())?;

        let entry = PlaintextLogEntry::IndexedEntry(EntryBody::OpBatch(OpBatch::new(
            timestamp,
            server_user_id,
            op_bytes,
        )));
        let entries = vec![entry];

        let next_chain_head = chain_head.compute_next_plaintext(&seed, &None, entries.iter())?;

        let sign_bytes = next_chain_head.sign_bytes(&seed);

        let signature = self.credentials.signing_key().sign(&sign_bytes)?;

        let segment = encode_segment_plaintext(&signature, &chain_head, &None, &entries)?;

        insert_cols_batch::<(
            segments::StreamId,
            segments::StartIdx,
            segments::EndSize,
            segments::Body,
        )>(
            batch.as_mut(),
            (stream_id, chain_head.size, next_chain_head.size, segment),
            Query::insert().into_table(segments::Table),
        )?;

        if commit_status == CommitStatus::Ok {
            // TODO does prepare indicate stall conditions?
            // somewhere in here maybe prepare, for ctl ops
            // we need to enrich them with observe & key wrap ops when needed
            // and also return a stall condition if waiting on another ctl
            // log from another peer
            let read_state = self
                .reducer
                .prepare(self.db.as_ref(), op)
                .await
                .map_err(|e| ExecError::Reducer(Box::new(e)))?;

            update_cols_batch::<(streams::HeadSize, streams::HeadHash, streams::CommitSize)>(
                batch.as_mut(),
                (
                    next_chain_head.size,
                    next_chain_head.hash,
                    next_chain_head.size,
                ), // head and commit sizes match
                Query::update()
                    .table(streams::Table)
                    .and_where(Expr::column(streams::Id).eq(stream_id)),
            )?;

            let apply_state = self
                .reducer
                .apply(batch.as_mut(), timestamp, op, read_state)
                .map_err(|e| ExecError::Reducer(Box::new(e)))?;

            let batch_result = batch.commit().await?;

            self.reducer
                .post_apply(apply_state, &batch_result)
                .map_err(|e| ExecError::Reducer(Box::new(e)))?;
        } else {
            // we cannot commit because our commit status is non-Ok, so we just update the head size and hash
            update_cols_batch::<(streams::HeadSize, streams::HeadHash)>(
                batch.as_mut(),
                (next_chain_head.size, next_chain_head.hash),
                Query::update()
                    .table(streams::Table)
                    .and_where(Expr::column(streams::Id).eq(stream_id)),
            )?;

            batch.commit().await?;

            // TODO should we return any kind of pending status to the caller? maybe not and this would be more of an alerting watch channel that could propogate to UI sync status or something
        }

        Ok(())
    }
}
