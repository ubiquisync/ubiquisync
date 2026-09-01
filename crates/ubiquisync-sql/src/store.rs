//! SQL-backed [`Store`](ubiquisync_core::store::Store) adding ad-hoc read queries.

use ubiquisync_core::event::RoutableEvent;

use crate::{
    db::{DbError, DbRow, DbValue},
    dialect::SqlDialect,
};

/// A [`Store`](ubiquisync_core::store::Store) backed by SQL, adding ad-hoc reads.
///
/// The inherited [`Store`](ubiquisync_core::store::Store) surface (`exec`) always
/// errors as the SQL engine's [`ProcessorError<BoxError>`], so it's pinned in the
/// supertrait bound rather than left as a parameter. [`query`](SqlStore::query)
/// reports its own [`DbError`] instead.
#[async_trait::async_trait]
pub trait SqlStore<Op, Event: RoutableEvent>:
    ubiquisync_core::store::Store<Op, ProcessorError<BoxError>, Event>
{
    /// Run a read-only query against the backend.
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError>;

    /// The backend's SQL dialect — lets a caller build dialect-correct SQL
    /// (placeholder style, quoting) to hand back to [`query`](SqlStore::query).
    fn dialect(&self) -> SqlDialect;
}
