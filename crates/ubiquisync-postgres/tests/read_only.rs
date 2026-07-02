//! Verifies the reader pool is genuinely read-only: a write routed through the
//! read path is rejected at the session level, not silently applied.

mod common;

use ubiquisync_sql::db::{Db, DbError};

#[tokio::test]
async fn reader_pool_rejects_writes() {
    let (_pg, db) = common::fresh_db().await;
    db.exec("CREATE TABLE t (id BIGINT PRIMARY KEY)", &[])
        .await
        .unwrap();

    // A write issued on the read path (`query` → reader pool) must fail the
    // session-level `default_transaction_read_only` guard with SQLSTATE 25006,
    // even though the embedded server's role is a superuser.
    let err = db
        .query("INSERT INTO t (id) VALUES (1)", &[])
        .await
        .unwrap_err();
    match err {
        DbError::Sql(msg) => assert!(
            msg.contains("25006"),
            "expected a read-only (25006) violation, got: {msg}"
        ),
        other => panic!("expected a Sql read-only error, got {other:?}"),
    }

    // Nothing was written, and the writer path still works.
    db.exec("INSERT INTO t (id) VALUES (1)", &[]).await.unwrap();
    let rows = db.query("SELECT COUNT(*) FROM t", &[]).await.unwrap();
    assert_eq!(rows[0].get_i64(0).unwrap(), 1);
}
