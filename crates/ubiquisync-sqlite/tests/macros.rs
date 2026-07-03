//! End-to-end check that `define_tables!` produces schemas the table
//! [`Reducer`] actually accepts: the generated `TableId`/`ColumnId`s must
//! materialize into physical storage and a user-facing VIEW under the declared
//! names. The macro's own unit tests only assert the in-memory `TableSchema`
//! shape; this closes the loop against a real `Db`.

use ubiquisync_sql::db::Db;
use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::reducer::Reducer;

// Declared at module scope, the way a real consumer would — a single-PK table
// and a composite-PK table, covering every column-type keyword.
ubiquisync_tables::define_tables! {
    notes:  1 ( pk: (id Uuid),         { (0, body, Text), (1, n, I64) } ),
    events: 2 ( pk: (k Text, seq I64), { (0, payload, Bytes) } ),
}

#[test]
fn generated_schemas_build_reducer_and_expose_views() {
    pollster::block_on(async {
        let db = SqliteDb::open_in_memory().unwrap();

        // The generated schemas must be accepted by the reducer, which creates
        // each table's physical storage and its user-facing VIEW up front.
        let _reducer = Reducer::new("app", &tables().unwrap(), &db)
            .await
            .expect("reducer accepts macro-generated schemas");

        // Selecting each declared column name from each VIEW proves the whole
        // chain wired up: the generated column IDs aliased back to the names
        // from the DSL. Empty result is expected — no rows written yet.
        let rows = db
            .query(r#"SELECT "id", "body", "n" FROM "notes""#, &[])
            .await
            .expect("notes view is queryable under its declared column names");
        assert!(rows.is_empty());

        let rows = db
            .query(r#"SELECT "k", "seq", "payload" FROM "events""#, &[])
            .await
            .expect("events view is queryable under its declared column names");
        assert!(rows.is_empty());
    });
}
