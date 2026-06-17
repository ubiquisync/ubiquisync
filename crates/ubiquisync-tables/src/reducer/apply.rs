use crate::db::Db;
use crate::op::Op;
use crate::reducer::{Reducer, ReducerError};
use crate::watch::ChangeEvent;
use ubiquisync_core::hlc::Timestamp;

impl Reducer {
    pub fn apply(
        &self,
        db: &dyn Db,
        timestamp: Timestamp,
        op: &Op,
    ) -> Result<Option<ChangeEvent>, ReducerError> {
        match op {
            Op::Upsert(upsert) => Ok(self
                .apply_upsert(db, timestamp, upsert)?
                .map(ChangeEvent::Upsert)),
            Op::Delete(delete) => Ok(self
                .apply_delete(db, timestamp, delete)?
                .map(ChangeEvent::Delete)),
        }
    }
}
