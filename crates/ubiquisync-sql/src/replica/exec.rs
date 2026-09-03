use std::time::Duration;

use backon::{ConstantBuilder, Retryable};
use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::SigningError,
    ids::{ContainerId, LogId},
    log::{
        ChainHash, ChainSeed, EntryBody, OpBatch, PlaintextLogEntry, SegmentCipherError,
        segment::{SegmentEncodeError, encode_segment_plaintext},
    },
    uuid::Uuid,
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols, insert_cols_batch, select_cols, update_cols_batch},
    },
    op::Op,
    reducer::Reducer,
    replica::{
        replica::Replica,
        schema::{segments, streams},
    },
};

impl<R: Reducer> Replica<R>
where
    R::Op: Op,
{
    /// Apply a local write, minting a fresh log entry for it.
    #[tracing::instrument(skip_all)]
    pub async fn exec(
        &self,
        server_user_id: Option<Uuid>,
        op: R::Op,
    ) -> Result<(), ExecError<R::Error>> {
        let (container_id, wire_bytes) = op.encode();
        (|| self.do_exec(server_user_id, &op, container_id, &wire_bytes))
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

    async fn do_exec(
        &self,
        server_user_id: Option<Uuid>,
        op: &R::Op,
        container_id: ContainerId,
        wire_bytes: &[u8],
    ) -> Result<(), ExecError<R::Error>> {
        // TODO does prepare indicate stall conditions?
        // somewhere in here maybe prepare, for ctl ops
        // we need to enrich them with observe & key wrap ops when needed
        // and also return a stall condition if waiting on another ctl
        // log from another peer
        let read_state = self
            .reducer
            .prepare(self.db.as_ref(), op)
            .await
            .map_err(ExecError::Reducer)?;

        // TODO we want a per stream mutex to avoid race conditions
        let stream_rows = select_cols::<(
            streams::Id,
            streams::HeadSize,
            streams::HeadHash,
            streams::HeadCipher,
            streams::HeadStatus,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(streams::Table)
                .and_where(Expr::column(streams::PeerId).eq(self.self_db_id))
                .and_where(Expr::column(streams::ContainerId).eq(container_id.as_ref())),
        )
        .await
        .map_err(ExecError::Db)?;

        let log_id = LogId {
            peer_id: self.self_id,
            container_id,
        };
        let seed = ChainSeed::new(&log_id);

        let (stream_id, chain_head) = if stream_rows.is_empty() {
            // TODO handle current inserts of root branch to stream!
            let res = insert_cols::<(streams::PeerId, streams::ContainerId), (streams::Id,)>(
                self.db.as_ref(),
                (self.self_db_id, container_id.0),
                Query::insert().into_table(streams::Table),
            )
            .await
            .map_err(ExecError::Db)?;
            let (stream_id,) = res.exactly_one().map_err(ExecError::Db)?;
            (stream_id, ChainHash::empty(&seed))
        } else if stream_rows.len() > 1 {
            todo!("found multiple rows, this means we have a fork and need to know what to do")
        } else {
            let (stream_id, head_size, head_hash, head_cipher, head_status) =
                stream_rows.exactly_one().map_err(ExecError::Db)?;

            if head_status.is_some() {
                todo!("handle some unexpected status")
            }

            if head_cipher.is_some() {
                todo!("cipher not supported yet");
            }

            let chain_head = ChainHash {
                hash: head_hash,
                size: head_size,
            };

            (stream_id, chain_head)
        };

        let mut batch = self.db.new_batch();
        let timestamp = self.hlc.now(batch.as_mut()).map_err(ExecError::Db)?;

        let entry = PlaintextLogEntry::IndexedEntry(EntryBody::OpBatch(OpBatch::new(
            timestamp,
            server_user_id,
            wire_bytes.to_vec(),
        )));
        let entries = vec![entry];

        let next_chain_head = chain_head
            .compute_next_plaintext(&seed, &None, entries.iter())
            .map_err(ExecError::SegmentCipher)?;

        let sign_bytes = next_chain_head.sign_bytes(&seed);

        let signature = self
            .credentials
            .signing_key()
            .sign(&sign_bytes)
            .map_err(ExecError::SigningError)?;

        let segment = encode_segment_plaintext(&signature, &chain_head, &None, &entries)
            .map_err(ExecError::SegmentEncode)?;

        insert_cols_batch::<(
            segments::StreamId,
            segments::StartIdx,
            segments::EndSize,
            segments::BodyId,
        )>(
            batch.as_mut(),
            (stream_id, chain_head.size, next_chain_head.size, todo!()),
            Query::insert().into_table(segments::Table),
        )
        .map_err(ExecError::Db)?;

        update_cols_batch::<(
            streams::HeadSize,
            streams::HeadHash,
            streams::NextSegmentSeq,
        )>(
            batch.as_mut(),
            (
                next_chain_head.size,
                Some(next_chain_head.hash.to_vec()),
                segment_seq + 1,
            ),
            Query::update()
                .table(streams::Table)
                .and_where(Expr::column(streams::Id).eq(stream_id)),
        )
        .map_err(ExecError::Db)?;

        self.reducer
            .apply(batch.as_mut(), timestamp, op, read_state)
            .map_err(ExecError::Reducer)?;

        batch.commit().await.map_err(ExecError::Db)?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum ExecError<E> {
    #[error("reducer error: {0}")]
    Reducer(E),
    #[error("unexpected error: {0}")]
    Internal(String),
    #[error("db error: {0}")]
    Db(DbError),
    #[error("segment cipher error: {0}")]
    SegmentCipher(SegmentCipherError),
    #[error("signing error: {0}")]
    SigningError(SigningError),
    #[error("segment encode error: {0}")]
    SegmentEncode(SegmentEncodeError),
}
