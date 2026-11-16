use sea_query::{Expr, ExprTrait, Query};
use ubiquisync_core::{
    ids::LogId,
    log::{
        ChainHash, EntryBody, LogHashContext, OpEntry, PlaintextLogEntry,
        segment::encode_segment_plaintext,
    },
    uuid::Uuid,
};

use crate::{
    Exec, ExecError,
    db::sea_query::{insert_cols, insert_cols_batch, update_cols_batch},
    reducer::Reducer,
    replica::{
        Replica,
        branches::LogKey,
        schema::{branches, segments},
    },
};

#[async_trait::async_trait]
impl<R: Reducer> Exec<R::Op> for Replica<R> {
    /// Apply a local write, minting a fresh log entry for it.
    #[tracing::instrument(skip_all)]
    async fn exec(&self, server_user_id: Option<Uuid>, op: R::Op) -> Result<(), ExecError> {
        let inner = self.inner.as_ref();
        let (container_id, op_bytes) = inner.reducer.codec().encode(&op)?;
        // per-log lock: only one task touches a log (all of its branches) at a time
        let log_guard = inner
            .log_locks
            .lock(&LogKey::new(inner.self_db_id, container_id))
            .await;

        let branch_rows = inner.resolve_branches(&log_guard).await?;

        let log_id = LogId {
            peer_id: inner.self_init.peer_id,
            container_id,
        };
        let seed = LogHashContext::new(&log_id);

        let (branch_id, chain_head, mut head_cipher, commit_err) = if branch_rows.is_empty() {
            let empty_chain = ChainHash::empty(&seed);
            let res = insert_cols::<
                (
                    branches::PeerId,
                    branches::ContainerId,
                    branches::HeadSize,
                    branches::HeadHash,
                    branches::CommitSize,
                ),
                (branches::Id,),
            >(
                inner.db.as_ref(),
                (inner.self_db_id, container_id.0, 0, empty_chain.hash, 0),
                Query::insert().into_table(branches::Table),
            )
            .await?;
            let (branch_id,) = res.exactly_one()?;
            (branch_id, empty_chain, None, None)
        } else if branch_rows.len() > 1 {
            todo!("found multiple rows, this means we have a fork and need to know what to do")
        } else {
            let branch = &branch_rows[0];

            if branch.head_err.is_some() {
                todo!("handle some unexpected status")
            }

            if branch.commit_err.is_none() && branch.commit_size != branch.head_chain.size {
                return Err(ExecError::Internal(format!(
                    "commit status is okay but head {} and commit {} sizes do not match",
                    branch.head_chain.size, branch.commit_size,
                )));
            }

            (
                branch.id,
                branch.head_chain,
                branch.head_cipher,
                branch.commit_err.clone(),
            )
        };

        let mut batch = inner.db.new_batch();
        let timestamp = inner.local_hlc(batch.as_mut())?;

        let entry = PlaintextLogEntry::IndexedEntry(EntryBody::Op(OpEntry::new(
            timestamp,
            server_user_id
                .map(|b| b.to_vec().into())
                .unwrap_or_default(),
            op_bytes,
        )));
        let entries = [entry];

        let next_chain_head = chain_head
            .compute_next_plaintext(
                &seed,
                &mut head_cipher,
                self.inner.key_resolver.as_ref(),
                entries.iter(),
            )
            .await?;

        let sign_bytes = next_chain_head.sign_bytes(&seed);

        let signature = inner.credentials.signing_key().sign(&sign_bytes)?;

        let segment = encode_segment_plaintext(
            &signature,
            &chain_head,
            &head_cipher,
            &log_id,
            inner.key_resolver.as_ref(),
            entries.iter(),
        )
        .await?;

        insert_cols_batch::<(
            segments::BranchId,
            segments::StartIdx,
            segments::EndSize,
            segments::Body,
        )>(
            batch.as_mut(),
            (branch_id, chain_head.size, next_chain_head.size, segment),
            Query::insert().into_table(segments::Table),
        )?;

        if commit_err.is_none() {
            // TODO does prepare indicate stall conditions?
            // somewhere in here maybe prepare, for ctl ops
            // we need to enrich them with observe & key wrap ops when needed
            // and also return a stall condition if waiting on another ctl
            // log from another peer
            let read_state = inner
                .reducer
                .prepare(inner.db.as_ref(), &op)
                .await
                .map_err(|e| ExecError::Reducer(Box::new(e)))?;

            update_cols_batch::<(
                branches::HeadSize,
                branches::HeadHash,
                branches::CommitSize,
                branches::HeadCipher,
            )>(
                batch.as_mut(),
                (
                    next_chain_head.size,
                    next_chain_head.hash,
                    next_chain_head.size,
                    head_cipher,
                ), // head and commit sizes match
                Query::update()
                    .table(branches::Table)
                    .and_where(Expr::column(branches::Id).eq(branch_id)),
            )?;

            let apply_state = inner
                .reducer
                .apply(batch.as_mut(), timestamp, &op, read_state)
                .map_err(|e| ExecError::Reducer(Box::new(e)))?;

            let batch_result = batch.commit().await?;

            inner.reducer.post_apply(apply_state, &batch_result);
        } else {
            // we cannot commit because our commit status is non-Ok, so we just update the head size and hash
            update_cols_batch::<(branches::HeadSize, branches::HeadHash, branches::HeadCipher)>(
                batch.as_mut(),
                (next_chain_head.size, next_chain_head.hash, head_cipher),
                Query::update()
                    .table(branches::Table)
                    .and_where(Expr::column(branches::Id).eq(branch_id)),
            )?;

            batch.commit().await?;

            // TODO should we return any kind of pending status to the caller? maybe not and this would be more of an alerting watch channel that could propogate to UI sync status or something
        }

        Ok(())
    }
}
