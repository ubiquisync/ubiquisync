use sea_query::{
    ColumnName, ColumnRef, DeleteStatement, Expr, InsertStatement, PostgresQueryBuilder,
    SelectStatement, SqliteQueryBuilder, UpdateStatement, Value, Values,
};

use crate::{
    db::{Cols, Db, DbBatch, DbError, DbValue, Rows},
    dialect::SqlDialect,
};

pub async fn select_cols<C: Cols>(
    db: &dyn Db,
    stmt: &mut SelectStatement,
) -> Result<Rows<C>, DbError> {
    stmt.columns(C::idens());
    let (sql, params) = build_select(stmt, db.dialect())?;
    let res = db.query(&sql, &params).await?;
    Ok(res.into())
}

pub async fn insert_cols<Inserting: Cols, Returning: Cols>(
    db: &dyn Db,
    params: Inserting::Params,
    stmt: &mut InsertStatement,
) -> Result<Rows<Returning>, DbError> {
    let (sql, values) = prep_insert_cols::<Inserting, Returning>(params, stmt, db.dialect())?;
    let res = db.query(&sql, &values).await?;
    Ok(res.into())
}

pub fn insert_cols_batch<C: Cols>(
    batch: &mut dyn DbBatch,
    params: C::Params,
    stmt: &mut InsertStatement,
) -> Result<(), DbError> {
    let (sql, values) = prep_insert_cols::<C, ()>(params, stmt, batch.dialect())?;
    batch.add_statement(&sql, &values);
    Ok(())
}

pub fn prep_insert_cols<Inserting: Cols, Returning: Cols>(
    params: Inserting::Params,
    stmt: &mut InsertStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    // inserting
    stmt.columns(Inserting::idens());
    let db_vals = Inserting::encode(params);
    let mut vals = vec![];
    for v in db_vals {
        vals.push(Expr::Constant(db_to_value(v)))
    }
    stmt.values(vals).map_err(|e| match e {
        sea_query::error::Error::ColValNumMismatch { col_len, val_len } => {
            DbError::ColumnValueCountMismatch {
                cols: col_len,
                vals: val_len,
            }
        }
    })?;

    // returning
    let returning_idens = Returning::idens();
    if !returning_idens.is_empty() {
        let mut col_refs = vec![];
        for i in returning_idens {
            col_refs.push(ColumnRef::Column(ColumnName(None, i)))
        }
        stmt.returning(sea_query::ReturningClause::Columns(col_refs));
    }

    build_insert(stmt, dialect)
}

pub fn update_cols_batch<C: Cols>(
    batch: &mut dyn DbBatch,
    params: C::Params,
    stmt: &mut UpdateStatement,
) -> Result<(), DbError> {
    let db_vals = C::encode(params);
    let idens = C::idens();
    let mut iden_exprs = vec![];
    for (i, v) in db_vals.into_iter().enumerate() {
        iden_exprs.push((idens[i].clone(), Expr::Constant(db_to_value(v))));
    }
    stmt.values(iden_exprs);
    let (sql, values) = build_update(stmt, batch.dialect())?;
    batch.add_statement(&sql, &values);
    Ok(())
}

pub fn build_insert(
    stmt: &InsertStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    Ok((sql, values_to_db(values)?))
}

pub fn build_update(
    stmt: &UpdateStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    Ok((sql, values_to_db(values)?))
}

pub fn build_select(
    stmt: &SelectStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    Ok((sql, values_to_db(values)?))
}

pub fn build_delete(
    stmt: &DeleteStatement,
    dialect: SqlDialect,
) -> Result<(String, Vec<DbValue>), DbError> {
    let (sql, values) = match dialect {
        SqlDialect::Sqlite => stmt.build_any(&SqliteQueryBuilder),
        SqlDialect::Postgres => stmt.build_any(&PostgresQueryBuilder),
    };
    Ok((sql, values_to_db(values)?))
}

fn values_to_db(values: Values) -> Result<Vec<DbValue>, DbError> {
    values
        .into_iter()
        .map(value_to_db)
        .collect::<Result<Vec<_>, _>>()
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

fn db_to_value(value: DbValue) -> Value {
    match value {
        DbValue::Null => Value::Bytes(None),
        DbValue::Integer(i) => Value::BigInt(Some(i)),
        DbValue::Text(s) => Value::String(Some(s)),
        DbValue::Blob(b) => Value::Bytes(Some(b)),
        DbValue::Uuid(u) => Value::Bytes(Some(u.into())),
    }
}
