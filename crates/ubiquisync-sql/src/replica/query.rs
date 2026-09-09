use crate::{
    SqlQueryStore,
    db::{DbError, DbRow, DbValue},
    replica::Replica,
};

#[async_trait::async_trait]
impl<R: Send + Sync> SqlQueryStore for Replica<R> {
    async fn query(&self, sql: &str, params: &[DbValue]) -> Result<Vec<DbRow>, DbError> {
        self.db.query(sql, params).await
    }

    fn dialect(&self) -> crate::dialect::SqlDialect {
        self.db.dialect()
    }
}
