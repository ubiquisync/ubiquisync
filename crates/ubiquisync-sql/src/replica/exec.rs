use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::{
    crypto::SigningError,
    hlc::Timestamp,
    ids::LogId,
    log::{
        ChainHash, ChainSeed, EntryBody, LogEntry, OpBatch, PlaintextLogEntry, SegmentCipherError,
        segment::{SegmentEncodeError, encode_segment_plaintext},
    },
    uuid::Uuid,
};

use crate::{
    db::{
        DbError,
        sea_query::{insert_cols_batch, select_cols, update_cols_batch},
    },
    op::Op,
    reducer::Reducer,
    replica::{
        replica::Replica,
        schema::{containers, peers, segments, streams},
    },
};

impl<R: Reducer> Replica<R>
where
    R::Op: Op,
{
    /// Apply a local write, minting a fresh log entry for it.
    async fn exec(
        // TODO do we want mut here or user interior mutability?
        // either way this makes exec single-threaded which we want for now (for safety of ordering)
        &mut self,
        server_user_id: Option<Uuid>,
        op: R::Op,
    ) -> Result<(), ExecError<R::Error>> {
        // TODO does prepare indicate stall conditions?
        // somewhere in here maybe prepare, for ctl ops
        // we need to enrich them with observe & key wrap ops when needed
        // and also return a stall condition if waiting on another ctl
        // log from another peer
        let read_state = self
            .reducer
            .prepare(self.db.as_ref(), &op)
            .await
            .map_err(ExecError::Reducer)?;

        let (container_id, wire_bytes) = op.encode();

        let stream_rows = select_cols::<(
            streams::Id,
            streams::HeadSize,
            streams::HeadHash,
            streams::HeadCipher,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(streams::Table)
                .and_where(
                    Expr::column(streams::PeerId).eq(Expr::col("id").in_subquery(
                        Query::select()
                            .from(peers::Table)
                            .column(peers::Id)
                            .and_where(Expr::column(peers::PeerId).eq(self.self_id.as_ref()))
                            .take(),
                    )),
                )
                .and_where(
                    Expr::column(streams::ContainerId).eq(Expr::col("id").in_subquery(
                        Query::select()
                            .from(containers::Table)
                            .column(containers::Id)
                            .and_where(
                                Expr::column(containers::ContainerId).eq(container_id.as_ref()),
                            )
                            .take(),
                    )),
                )
                .and_where(Expr::column(streams::HeadStatus).is_null()),
        )
        .await
        .map_err(ExecError::Db)?;

        if stream_rows.is_empty() {
            todo!("create stream")
        } else if stream_rows.len() > 1 {
            todo!("fork")
        } else {
            let (stream_id, head_size, head_hash, head_cipher) =
                stream_rows.exactly_one().map_err(ExecError::Db)?;
            if head_cipher.is_some() {
                todo!("cipher not supported");
            }

            let log_id = LogId {
                peer_id: self.self_id,
                container_id,
            };

            let seed = ChainSeed::new(&log_id);
            let chain_head = if let Some(hash) = head_hash {
                ChainHash {
                    hash: hash.try_into().map_err(|_| {
                        ExecError::Internal(format!("unexpected hash size {0}", hash.len()))
                    })?,
                    size: head_size,
                }
            } else {
                if head_size != 0 {
                    return Err(ExecError::Internal(format!(
                        "stream {stream_id} has empty head hash but head size {head_size}"
                    )));
                }
                ChainHash::empty(&seed)
            };

            let mut batch = self.db.new_batch();
            let timestamp = self.hlc.now(batch.as_mut()).map_err(ExecError::Db)?;

            let entry = PlaintextLogEntry::IndexedEntry(EntryBody::OpBatch(OpBatch::new(
                timestamp,
                server_user_id,
                wire_bytes,
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
                segments::Body,
            )>(
                batch.as_mut(),
                (stream_id, chain_head.size, next_chain_head.size, segment),
                Query::insert().into_table(segments::Table),
            )
            .map_err(ExecError::Db)?;

            update_cols_batch::<(streams::HeadSize, streams::HeadHash)>(
                batch.as_mut(),
                (next_chain_head.size, Some(next_chain_head.hash.to_vec())),
                Query::update()
                    .table(streams::Table)
                    .and_where(Expr::column(streams::Id).eq(stream_id)),
            )
            .map_err(ExecError::Db)?;

            self.reducer
                .apply(batch.as_mut(), timestamp, op, read_state)
                .map_err(ExecError::Reducer)?;

            Ok(())
        }
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
