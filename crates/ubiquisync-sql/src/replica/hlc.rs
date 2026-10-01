use std::{convert::Into, sync::atomic::Ordering};

use sea_query::{Alias, Expr, ExprTrait, OnConflict, Query};
use thiserror::Error;
use ubiquisync_core::hlc::{Timestamp, TimestampOverflow, WallTime};

use crate::{
    db::{
        Db, DbBatch, DbError,
        sea_query::{insert_cols_batch, select_cols},
    },
    replica::{Replica, schema::hlc},
};

#[derive(Error, Debug)]
pub enum HlcError {
    #[error("db error: {0}")]
    Db(#[from] DbError),
    #[error("timestamp overflow")]
    Overflow(#[from] TimestampOverflow),
    #[error("clock seq, remote: {remote:?}, local: {local:?}")]
    Skew { remote: Timestamp, local: WallTime },
}

const MAX_SKEW_MS: u64 = 60_000;
impl<R> Replica<R> {
    pub(crate) async fn load_hlc(db: &dyn Db) -> Result<Timestamp, HlcError> {
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

    pub(crate) fn local_hlc(&self, batch: &mut dyn DbBatch) -> Result<Timestamp, HlcError> {
        let wall_ms = WallTime::now();
        self.update_hlc(batch, |ts| ts.next_local(wall_ms))
    }

    pub(crate) fn observe_remote_hlc(
        &self,
        batch: &mut dyn DbBatch,
        remote: Timestamp,
    ) -> Result<(), HlcError> {
        let local = WallTime::now();
        let remote_ms = remote.wall();
        if remote_ms > local && remote_ms.as_millis() - local.as_millis() > MAX_SKEW_MS {
            return Err(HlcError::Skew { remote, local });
        }
        let next = remote.next_local(local)?;
        self.update_hlc(batch, |ts| ts.next_after(next))?;
        Ok(())
    }

    fn update_hlc<F>(&self, batch: &mut dyn DbBatch, f: F) -> Result<Timestamp, HlcError>
    where
        F: Fn(Timestamp) -> Result<Timestamp, TimestampOverflow>,
    {
        let mut last = self.hlc.load(Ordering::Relaxed);
        loop {
            let next = f(Timestamp::try_from(last).expect("valid timestamp"))?;
            match self.hlc.compare_exchange_weak(
                last,
                next.into(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.persist_hlc(next, batch)?;
                    return Ok(next);
                }
                Err(cur) => last = cur,
            }
        }
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
