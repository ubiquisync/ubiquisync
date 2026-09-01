use thiserror::Error;
use ubiquisync_core::uuid::Uuid;

use crate::{db::DbError, op::Op, reducer::Reducer, replica::replica::Replica};

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
        self.reducer
            .prepare(self.db.as_ref(), &op)
            .await
            .map_err(ExecError::Reducer)?;
        // somewhere in here maybe prepare, for ctl ops
        // we need to enrich them with observe & key wrap ops when needed
        // and also return a stall condition if waiting on another ctl
        // log from another peer
        let (container, wire_bytes) = op.encode();

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
