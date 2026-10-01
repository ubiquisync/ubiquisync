use std::{convert::Into, sync::atomic::Ordering};

use sea_query::{Alias, Expr, ExprTrait, OnConflict, Query};
use ubiquisync_core::hlc::{Timestamp, WallTime};

use crate::{
    db::{
        Db, DbBatch, DbError,
        sea_query::{insert_cols_batch, select_cols},
    },
    replica::{Replica, schema::hlc},
};

const MAX_SKEW_MS: u64 = 60_000;
impl<R> Replica<R> {
    pub(crate) async fn load_hlc(db: &dyn Db) -> Result<Timestamp, DbError> {
        let Some((ts,)) = select_cols::<(hlc::Timestamp,)>(
            db,
            Query::select()
                .from(hlc::Table)
                .and_where(Expr::column(hlc::Id).eq(1)),
        )
        .await?
        .one()?
        else {
            return Ok(Timestamp::default());
        };
        Ok(ts)
    }

    pub(crate) fn local_hlc(&self, batch: &mut dyn DbBatch) -> Result<Timestamp, DbError> {
        let wall_ms = WallTime::now();
        let last_hlc = self.hlc.load(Ordering::Relaxed);
        let next_hlc = Timestamp::try_from(last_hlc)
            .ok()
            .and_then(|ts| ts.next_local(wall_ms).ok())
            .expect("valid timestamp");
        self.hlc.store(next_hlc.into(), Ordering::Relaxed);
        self.persist_hlc(next_hlc, batch)?;
        Ok(next_hlc)
    }

    fn persist_hlc(&self, ts: Timestamp, batch: &mut dyn DbBatch) -> Result<(), DbError> {
        insert_cols_batch::<(hlc::Id, hlc::Timestamp)>(
            batch,
            (1, ts),
            Query::insert().into_table(hlc::Table).on_conflict(
                OnConflict::column(hlc::Id)
                    .update_column(hlc::Timestamp)
                    .action_and_where(
                        Expr::col((Alias::new("excluded"), hlc::Timestamp))
                            .gt(Expr::col((hlc::Table, hlc::Timestamp))),
                    )
                    .to_owned(),
            ),
        )
    }
}
