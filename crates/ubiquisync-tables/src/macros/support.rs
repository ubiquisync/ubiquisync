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
pub use pastey;
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

/// Map a sea-query bind [`Value`] to our [`DbValue`]. The `get`/`get_all` readers
/// only feed macro-controlled binds, but `query` hands callers the full
/// [`Expr`](sea_query::Expr) surface, so any scalar can arrive here. Booleans,
/// every signed/unsigned integer width (`LIMIT`/`OFFSET` bind unsigned), chars,
/// strings, bytes, and UUIDs map cleanly; a float or any exotic type has no
/// `DbValue` (our columns are only `Bytes`/`Uuid`/`Text`/`I64`), so it's a caller
/// type error reported as [`DbError`] — never a panic.
pub fn value_to_db(value: Value) -> Result<DbValue, DbError> {
    let db = match value {
        Value::Bool(Some(b)) => DbValue::Integer(b as i64),
        Value::TinyInt(Some(i)) => DbValue::Integer(i as i64),
        Value::SmallInt(Some(i)) => DbValue::Integer(i as i64),
        Value::Int(Some(i)) => DbValue::Integer(i as i64),
        Value::BigInt(Some(i)) => DbValue::Integer(i),
        Value::TinyUnsigned(Some(u)) => DbValue::Integer(u as i64),
        Value::SmallUnsigned(Some(u)) => DbValue::Integer(u as i64),
        Value::Unsigned(Some(u)) => DbValue::Integer(u as i64),
        Value::BigUnsigned(Some(u)) => {
            DbValue::Integer(i64::try_from(u).map_err(|_| DbError::IntegerOutOfRange(u as i128))?)
        }
        Value::Char(Some(c)) => DbValue::Text(c.to_string()),
        Value::String(Some(s)) => DbValue::Text(s),
        Value::Bytes(Some(b)) => DbValue::Blob(b),
        Value::Uuid(Some(u)) => DbValue::Uuid(u.into_bytes()),
        Value::Bool(None)
        | Value::TinyInt(None)
        | Value::SmallInt(None)
        | Value::Int(None)
        | Value::BigInt(None)
        | Value::TinyUnsigned(None)
        | Value::SmallUnsigned(None)
        | Value::Unsigned(None)
        | Value::BigUnsigned(None)
        | Value::Char(None)
        | Value::String(None)
        | Value::Bytes(None)
        | Value::Uuid(None) => DbValue::Null,
        other => {
            return Err(DbError::Sql(format!(
                "unsupported value bound into query: {other:?}"
            )));
        }
    };
    Ok(db)
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
pub fn build_select(
    stmt: &SelectStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    let params = values
        .into_iter()
        .map(value_to_db)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((sql, params))
}
