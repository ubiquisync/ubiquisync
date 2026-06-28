use ubiquisync_core::{
    hlc::{HlcError, HlcService, HlcStorage},
    log_entry::LogEntry,
};

use crate::{
    db::{Db, DbError},
    hlc_storage::SqlHlcStorage,
    reducer::Reducer,
};

pub struct Processor<R: Reducer, D: Db> {
    reducer: R,
    db: D,
    hlc: HlcService<SqlHlcStorage>,
}

impl<R: Reducer, D: Db> Processor<R, D> {
    async fn process_one(
        &mut self,
        entry: LogEntry<R::Op>,
    ) -> Result<R::Event, ProcessorError<R::Error>> {
        let op = &entry.op;
        let timestamp = entry.timestamp;
        let prepare_state = self.reducer.prepare(db, op).await?;
        let mut batch = self.db.new_batch();
        self.hlc.observe(timestamp, wall_ms(), batch.as_mut())?;
        let apply_state = self
            .reducer
            .apply(batch.as_mut(), timestamp, op, prepare_state)?;
        let batch_result = batch.commit()?;
        let event = self.reducer.post_apply(apply_state, batch_result)?;
        Ok(event)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessorError<E> {
    #[error("reducer error: {0}")]
    ReducerError(#[from] E),
    #[error("hlc error: {0}")]
    HlcError(#[from] HlcError<DbError>),
    #[error("db error: {0}")]
    DbError(#[from] DbError),
}
