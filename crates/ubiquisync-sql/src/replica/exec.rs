use ::sea_query::Expr;
use sea_query::{Query, SelectStatement};
use thiserror::Error;
use ubiquisync_core::{log_entry::EncodableOp, uuid::Uuid};

use crate::{
    db::{DbError, DbRow, sea_query::{self, select}},
    reducer::Reducer,
    replica::{replica::Replica, schema::Streams},
};

impl<R: Reducer> Replica<R>
where
    R::Op: EncodableOp,
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

        let res = select(self.db.as_ref(), Query::select()
            .column(Streams::HeadIdx)
            .column(Streams::HeadHash)
            .column(Streams::HeadCipher)
            .from(Streams::Table)
            .and_where(Expr::column(Streams::PeerId))
        ).await?;

        // somewhere in here maybe prepare, for ctl ops
        // we need to enrich them with observe & key wrap ops when needed
        // and also return a stall condition if waiting on another ctl
        // log from another peer
        let (container, wire_bytes) = op.encode();

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
