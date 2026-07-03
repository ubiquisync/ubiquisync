//! SQL-backed [`Store`](ubiquisync_core::store::Store) adding ad-hoc read queries.

use ubiquisync_core::event::RoutableEvent;

use crate::db::{DbError, DbRow, DbValue};

/// A [`Store`](ubiquisync_core::store::Store) backed by SQL, adding ad-hoc reads.
#[async_trait::async_trait]
pub trait SqlStore<Op, Err, Event: RoutableEvent>:
    ubiquisync_core::store::Store<Op, Err, Event>
{
    /// Run a read-only query against the backend.
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError>;
}
