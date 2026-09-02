use sea_query::{Expr, ExprTrait, Query};
use thiserror::Error;
use ubiquisync_core::uuid::Uuid;

use crate::{
    db::{DbError, DbRow, sea_query::select}, op::Op, reducer::Reducer, replica::{replica::Replica, schema::Streams}
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

        let stream_rows = select(self.db.as_ref(), Query::select()
            .column(Streams::HeadIdx)
            .column(Streams::HeadHash)
            .column(Streams::HeadCipher)
            .from(Streams::Table)
            .and_where(Expr::column(Streams::PeerId).eq(self.self_id.as_ref()))
            .and_where(Expr::column(Streams::ContainerId).eq(container.as_ref()))
        ).await?;

        if stream_rows.is_empty() {
            todo!("create stream")
        } else if stream_rows.len() > 1 {
            todo!("fork")
        } else {
            let stream_row = &stream_rows[0];
            let head_idx = stream_row.get_u64(0)?;
            let head_hash = stream_row.get_blob(1)?;
            let head_cipher = stream_row.get_optional_blob(2)?;
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

    fn select(&mut self, select: SelectStatement) -> Result<Vec<DbRow>, DbError> {

        self.db
            .query(&, params)
    }
}

#[derive(Error, Debug)]
pub enum ExecError<E> {
    #[error("reducer error: {0}")]
    Reducer(E),
    #[error("db error: {0}")]
    Db(#[from] DbError),
}
