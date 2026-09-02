use sea_query::{PostgresQueryBuilder, SelectStatement, SqliteQueryBuilder, Value};

use crate::{
    db::{Cols, Db, DbError, DbRow, DbValue, Rows},
    dialect::SqlDialect,
};

pub async fn select(db: &dyn Db, stmt: &SelectStatement) -> Result<Vec<DbRow>, DbError> {
    let (sql, params) = build_select(stmt, db.dialect())?;
    let res = db.query(&sql, &params).await?;
    Ok(res)
}

pub async fn select_cols<C: Cols>(
    db: &dyn Db,
    mut stmt: SelectStatement,
) -> Result<Rows<C>, DbError> {
    C::add_to_select(&mut stmt);
    let (sql, params) = build_select(&stmt, db.dialect())?;
    let res = db.query(&sql, &params).await?;
    Ok(res.into())
}

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

fn value_to_db(value: Value) -> Result<DbValue, DbError> {
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
