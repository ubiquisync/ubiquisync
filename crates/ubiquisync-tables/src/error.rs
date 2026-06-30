use ubiquisync_sql::db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum TablesError {
    #[error("db error: {0}")]
    DbError(#[from] DbError),
}
