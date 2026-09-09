//! Internal glue the table-definition macros expand against.
//!
//! Re-exports the third-party and sibling-crate paths generated code needs (so a
//! downstream crate calling [`define_tables!`](crate::define_tables) needn't
//! depend on `sea-query`/`uuid`/`ubiquisync-sql` directly).
//!
//! Not part of the public API — everything here is an implementation detail of
//! the macros and may change without notice.
#![allow(missing_docs)]

pub use sea_query;
pub use uuid;

pub use futures::Stream;
pub use pastey;
pub use ubiquisync_core::event::{RoutableEvent, Subscribe};
pub use ubiquisync_core::uuid::Uuid as CoreUuid;
pub use ubiquisync_sql::SqlQueryStore;
pub use ubiquisync_sql::db::sea_query::build_sql;
pub use ubiquisync_sql::db::{DbError, DbRow, DbValue};

use futures::{StreamExt, future::ready};
use ubiquisync_core::event::Subscription;

use crate::col_type::ColType;
use crate::id::ColumnId;
use crate::op::{ColumnSet, Value as OpValue};
use crate::watch::ChangeEvent;

/// Record one column write on an upsert builder: `Some(value)` sets the column,
/// `None` sets it to SQL NULL. Shared by the generated typed setters.
pub fn push_col(
    sets: &mut Vec<ColumnSet>,
    nulls: &mut Vec<ColumnId>,
    index: u8,
    col_type: ColType,
    value: Option<OpValue>,
) {
    let column_id = ColumnId::new(index, col_type);
    match value {
        Some(value) => sets.push(ColumnSet { column_id, value }),
        None => nulls.push(column_id),
    }
}

/// Project a raw [`ChangeEvent`] subscription into a stream of a table's typed
/// event `T`, dropping events for other tables (those `T::try_from` rejects).
pub fn project_events<T>(sub: Subscription<ChangeEvent>) -> impl Stream<Item = T>
where
    T: TryFrom<ChangeEvent>,
{
    sub.filter_map(|event| ready(T::try_from(event).ok()))
}
