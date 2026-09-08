use crate::{
    db::{DbError, DbRow, DbValue},
    replica::replica::Replica,
};

impl<R> Replica<R> {
    pub async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        self.db.query(sql, params).await
    }

    pub fn dialect(&self) -> crate::dialect::SqlDialect {
        self.db.dialect()
    }
}
