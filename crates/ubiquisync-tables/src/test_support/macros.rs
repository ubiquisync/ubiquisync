//! Backend-agnostic suite for the `define_tables!` macro surface, driven through
//! a real [`Processor`] over the caller's [`Db`]. Exercises every generated
//! helper — the `upsert`/`delete` op builders and the `get`/`get_all`/`query`/
//! `watch` readers — so the whole path (op → reducer → physical table → VIEW →
//! typed row/event) is asserted against a real backend.
//!
//! Scenarios share one store (like the other suites) and stay isolated by using
//! disjoint keys: point/watch scenarios use distinct `notes` PKs, `counters` is
//! reserved for the `get_all` scan, and the `events` query scenario scopes itself
//! with a `k` filter.

use futures::{FutureExt, StreamExt};
use ubiquisync_core::event::EventBus;
use ubiquisync_sql::db::Db;
use ubiquisync_sql::processor::Processor;
use ubiquisync_sql::store::SqlStore;
use ubiquisync_sql::tracker::LogIndexTracker;

use crate::op::Op;
use crate::reducer::Reducer;
use crate::sea_query::{Expr, ExprTrait, Order};
use crate::watch::{ChangeEvent, ColumnChange};

// A single-PK table covering all four column types, a composite-PK table, and a
// key-only table (empty value-column list).
crate::define_tables! {
    1 notes    (id Uuid)         => { (0 body Text), (1 n I64), (2 blob Bytes), (3 who Uuid) },
    2 events   (k Text, seq I64) => { (0 payload Bytes) },
    3 counters (id I64)          => {},
}

const NODE: [u8; 16] = [1u8; 16];

/// Run every macro-surface scenario against `db`. Call with a freshly opened,
/// empty database.
pub async fn run_macros_suite<D: Db>(db: D) {
    let store = open(db).await;
    let s = &store;

    upsert_roundtrips_every_column_type(s).await;
    missing_row_reads_as_none(s).await;
    later_upsert_wins_last_writer(s).await;
    null_write_clears_the_column(s).await;
    delete_hides_the_row(s).await;
    get_all_returns_live_rows_only(s).await;
    query_filters_orders_and_limits(s).await;
    composite_pk_upsert_get_and_delete(s).await;
    watch_delivers_typed_upsert_event(s).await;
    watch_reports_a_null_write_as_set_null(s).await;
    watch_row_only_sees_its_own_row(s).await;
    watch_delivers_delete_event(s).await;
}

/// Open the store under test: the table reducer over `db`, behind `SqlStore`.
async fn open<D: Db>(db: D) -> impl SqlStore<Op, ChangeEvent> {
    let reducer = Reducer::new("app", &tables().expect("schemas build"), &db)
        .await
        .expect("reducer accepts schemas");
    Processor::<_, _, LogIndexTracker<Op>, EventBus<ChangeEvent>>::open(reducer, db, "app", NODE)
        .await
        .expect("processor opens")
}

// ── Writes + point reads ─────────────────────────────────────────────────────

async fn upsert_roundtrips_every_column_type(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x10; 16];
    let who = [0x1a; 16];

    s.exec(None, notes::upsert(&id).body("hello").n(42).blob(&[1, 2, 3]).who(&who).build())
        .await
        .unwrap();

    let row = notes::get(s, &id).await.unwrap().expect("row exists");
    assert_eq!(row.id, id);
    assert_eq!(row.body.as_deref(), Some("hello"));
    assert_eq!(row.n, Some(42));
    assert_eq!(row.blob.as_deref(), Some(&[1, 2, 3][..]));
    assert_eq!(row.who, Some(who));
}

async fn missing_row_reads_as_none(s: &impl SqlStore<Op, ChangeEvent>) {
    assert!(notes::get(s, &[0x20; 16]).await.unwrap().is_none());
}

async fn later_upsert_wins_last_writer(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x30; 16];
    // Each exec mints a strictly-later HLC timestamp, so the second write wins.
    s.exec(None, notes::upsert(&id).body("first").build()).await.unwrap();
    s.exec(None, notes::upsert(&id).body("second").build()).await.unwrap();

    let row = notes::get(s, &id).await.unwrap().unwrap();
    assert_eq!(row.body.as_deref(), Some("second"));
}

async fn null_write_clears_the_column(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x40; 16];
    s.exec(None, notes::upsert(&id).body("x").n(5).build()).await.unwrap();
    s.exec(None, notes::upsert(&id).n_null().build()).await.unwrap();

    let row = notes::get(s, &id).await.unwrap().unwrap();
    assert_eq!(row.body.as_deref(), Some("x")); // untouched column survives
    assert_eq!(row.n, None); // explicitly nulled
}

async fn delete_hides_the_row(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x50; 16];
    s.exec(None, notes::upsert(&id).body("doomed").build()).await.unwrap();
    assert!(notes::get(s, &id).await.unwrap().is_some());

    s.exec(None, notes::delete(&id)).await.unwrap();
    assert!(notes::get(s, &id).await.unwrap().is_none());
}

// ── Scans + filtered queries ─────────────────────────────────────────────────

async fn get_all_returns_live_rows_only(s: &impl SqlStore<Op, ChangeEvent>) {
    // `counters` is used only here, so `get_all` sees exactly these rows.
    for id in [1, 2, 3] {
        s.exec(None, counters::upsert(id).build()).await.unwrap();
    }
    s.exec(None, counters::delete(2)).await.unwrap();

    let mut ids: Vec<_> = counters::get_all(s).await.unwrap().into_iter().map(|r| r.id).collect();
    ids.sort();
    assert_eq!(ids, vec![1, 3]); // 2 was tombstoned
}

async fn query_filters_orders_and_limits(s: &impl SqlStore<Op, ChangeEvent>) {
    for seq in [1, 2, 3, 4] {
        s.exec(None, events::upsert("q", seq).payload(&[seq as u8]).build()).await.unwrap();
    }

    // Scoped to k="q" (isolating from the composite scenario's k="chan"), seq >= 2,
    // newest first, at most two rows.
    let rows = events::query(s, |b| {
        b.and_where(Expr::col(events::Col::k).eq("q"))
            .and_where(Expr::col(events::Col::seq).gte(2))
            .order_by(events::Col::seq, Order::Desc)
            .limit(2);
    })
    .await
    .unwrap();

    let seqs: Vec<_> = rows.iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![4, 3]);
}

async fn composite_pk_upsert_get_and_delete(s: &impl SqlStore<Op, ChangeEvent>) {
    s.exec(None, events::upsert("chan", 5).payload(&[9, 9]).build()).await.unwrap();

    let row = events::get(s, "chan", 5).await.unwrap().expect("row exists");
    assert_eq!(row.k, "chan");
    assert_eq!(row.seq, 5);
    assert_eq!(row.payload.as_deref(), Some(&[9, 9][..]));

    s.exec(None, events::delete("chan", 5)).await.unwrap();
    assert!(events::get(s, "chan", 5).await.unwrap().is_none());
}

// ── Watch: typed change events ───────────────────────────────────────────────

async fn watch_delivers_typed_upsert_event(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x60; 16];
    let mut w = notes::watch(s); // subscribe before the write

    s.exec(None, notes::upsert(&id).body("hi").build()).await.unwrap();

    // exec publishes synchronously, so the event is already buffered.
    match w.next().now_or_never().flatten().expect("event delivered") {
        notes::Event::Upsert(u) => {
            assert_eq!(u.id, id);
            assert_eq!(u.body, ColumnChange::Set("hi".to_string())); // set to "hi"
            assert_eq!(u.n, ColumnChange::Unchanged); // untouched by this write
        }
        other => panic!("expected upsert, got {other:?}"),
    }
}

async fn watch_reports_a_null_write_as_set_null(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x61; 16];
    s.exec(None, notes::upsert(&id).n(1).build()).await.unwrap();

    let mut w = notes::watch(s); // subscribe, then null the column
    s.exec(None, notes::upsert(&id).n_null().build()).await.unwrap();

    match w.next().now_or_never().flatten().expect("event delivered") {
        notes::Event::Upsert(u) => assert_eq!(u.n, ColumnChange::SetNull),
        other => panic!("expected upsert, got {other:?}"),
    }
}

async fn watch_row_only_sees_its_own_row(s: &impl SqlStore<Op, ChangeEvent>) {
    let (a, b) = ([0x70; 16], [0x71; 16]);
    let mut w = notes::watch_row(s, &a); // scoped to row A

    s.exec(None, notes::upsert(&b).body("other").build()).await.unwrap();
    assert!(
        w.next().now_or_never().flatten().is_none(),
        "a change to row B must not reach row A's watcher",
    );

    s.exec(None, notes::upsert(&a).body("mine").build()).await.unwrap();
    match w.next().now_or_never().flatten().expect("A's event") {
        notes::Event::Upsert(u) => assert_eq!(u.id, a),
        other => panic!("expected upsert, got {other:?}"),
    }
}

async fn watch_delivers_delete_event(s: &impl SqlStore<Op, ChangeEvent>) {
    let id = [0x80; 16];
    s.exec(None, notes::upsert(&id).body("x").build()).await.unwrap();

    let mut w = notes::watch(s); // subscribe after the upsert
    s.exec(None, notes::delete(&id)).await.unwrap();

    match w.next().now_or_never().flatten().expect("delete delivered") {
        notes::Event::Delete(d) => assert_eq!(d.id, id),
        other => panic!("expected delete, got {other:?}"),
    }
}
