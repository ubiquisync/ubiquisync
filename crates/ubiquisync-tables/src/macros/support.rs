//! Internal glue the table-definition macros expand against.
//!
//! Re-exports the third-party and sibling-crate paths generated code needs (so a
//! downstream crate calling [`define_tables!`](crate::define_tables) needn't
//! depend on `sea-query`/`uuid`/`ubiquisync-sql` directly), plus the small
//! runtime that turns a sea-query statement into the `(sql, params)` pair the
//! [`SqlStore`](ubiquisync_sql::store::SqlStore) read path wants.
//!
//! Not part of the public API — everything here is an implementation detail of
//! the macros and may change without notice.
#![allow(missing_docs)]

pub use sea_query;
pub use uuid;

pub use futures::Stream;
pub use paste;
pub use ubiquisync_core::event::RoutableEvent;
pub use ubiquisync_core::uuid::Uuid as CoreUuid;
pub use ubiquisync_sql::db::{DbError, DbRow, DbValue};
pub use ubiquisync_sql::store::SqlStore;

use futures::{StreamExt, future::ready};
use sea_query::{PostgresQueryBuilder, SelectStatement, SqliteQueryBuilder, Value};
use ubiquisync_core::event::Subscription;
use ubiquisync_sql::dialect::SqlDialect;

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

/// Map a sea-query bind [`Value`] to our [`DbValue`]. Only the variants the table
/// column types can produce are handled; any other variant is a codegen bug, not
/// a runtime input, so it panics.
pub fn value_to_db(value: Value) -> DbValue {
    match value {
        Value::BigInt(Some(i)) => DbValue::Integer(i),
        Value::Int(Some(i)) => DbValue::Integer(i as i64),
        Value::String(Some(s)) => DbValue::Text(s),
        Value::Bytes(Some(b)) => DbValue::Blob(b),
        Value::Uuid(Some(u)) => DbValue::Uuid(u.into_bytes()),
        // `LIMIT`/`OFFSET` bind as unsigned; they stay well within `i64`.
        Value::BigUnsigned(Some(u)) => DbValue::Integer(u as i64),
        Value::Unsigned(Some(u)) => DbValue::Integer(u as i64),
        Value::BigInt(None)
        | Value::Int(None)
        | Value::BigUnsigned(None)
        | Value::Unsigned(None)
        | Value::String(None)
        | Value::Bytes(None)
        | Value::Uuid(None) => DbValue::Null,
        other => panic!("ubiquisync-tables: unsupported bind value: {other:?}"),
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

/// Render `stmt` to dialect-correct SQL plus its ordered params, ready to hand to
/// [`SqlStore::query`](ubiquisync_sql::store::SqlStore::query). The dialect fixes
/// placeholder style (`?` vs `$1`) and
/// identifier quoting.
pub fn build_select(stmt: &SelectStatement, dialect: SqlDialect) -> (String, Vec<DbValue>) {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    (sql, values.into_iter().map(value_to_db).collect())
}
