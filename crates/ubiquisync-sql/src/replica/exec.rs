use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::uuid::Uuid;

use crate::{
    db::{
        DbError,
        sea_query::{select, select_cols},
    },
    op::Op,
    reducer::Reducer,
    replica::{replica::Replica, schema::streams},
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
        self.reducer
            .prepare(self.db.as_ref(), &op)
            .await
            .map_err(ExecError::Reducer)?;

        let (container, wire_bytes) = op.encode();

        let stream_rows = select_cols::<(
            streams::Id,
            streams::HeadIdx,
            streams::HeadHash,
            streams::HeadCipher,
        )>(
            self.db.as_ref(),
            Query::select()
                .from(streams::Table)
                .and_where(Expr::column(streams::PeerId).eq(self.self_id.as_ref()))
                .and_where(Expr::column(streams::ContainerId).eq(container.as_ref()))
                .and_where(Expr::column(streams::HeadStatus).is_null()),
        )
        .await?;

        if stream_rows.is_empty() {
            todo!("create stream")
        } else if stream_rows.len() > 1 {
            todo!("fork")
        } else {
            let (stream_id, head_idx, head_hash, head_cipher) = stream_rows.exactly_one()?;
            if head_cipher.is_some() {
                todo!("cipher not supported");
            }
        }

        // somewhere in here maybe prepare, for ctl ops
        // we need to enrich them with observe & key wrap ops when needed
        // and also return a stall condition if waiting on another ctl
        // log from another peer

        todo!()
    }
}

#[derive(Error, Debug)]
pub enum ExecError<E> {
    #[error("reducer error: {0}")]
    Reducer(E),
    #[error("db error: {0}")]
    Db(#[from] DbError),
}
