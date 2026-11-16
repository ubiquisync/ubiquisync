//! A replica whose commits stall keeps storing what it syncs, and the commit retry loop
//! applies it once the cause is gone.

use std::{
    collections::HashMap,
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use secrecy::SecretBox;
use test_case::test_case;
use ubiquisync_core::{
    crypto::credentials::software::SoftwareCredentials,
    event::event_bus,
    hlc::Timestamp,
    ids::{AppId, ContainerId, PeerId},
    pack::StdFsRemoteProvider,
};
use ubiquisync_sql::{
    Exec, SqlQueryStore,
    db::{Db, DbBatch, DbStatementResult, DbValue},
    op::OpCodec,
    reducer::{ApplyError, PrepareError, Reducer as SqlReducer},
    replica::{
        CommitRetryConfig, PackSyncConfig, Replica, ReplicaConfig, test_support::StreamHeads,
    },
};
use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::{
    col_type::ColType,
    id::{ColumnId, TableId},
    op::{ColumnSet, Op, Upsert, Value},
    reducer::Reducer,
    schema::{ColumnSchema, TableSchema},
};

const APP_ID: AppId = AppId([7; 16]);
const CONTAINER_ID: ContainerId = ContainerId([9; 16]);
const TABLE: TableId = TableId::new(&[ColType::I64], 1);
const COL_V: ColumnId = ColumnId::new(0, ColType::Text);
const FOLDER: &str = "folder";
const TICK: Duration = Duration::from_millis(20);

#[test_case(Failure::Prepare, TICK, false ; "once the backoff passes")]
#[test_case(Failure::Prepare, Duration::from_hours(1), true ; "right after a restart, whatever the backoff")]
#[test_case(Failure::DbWrite, Duration::from_hours(1), true ; "a failed db write backs off too")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_commits_catch_up(failure: Failure, retry_delay: Duration, restart: bool) {
    let root = temp_root(&format!("stall-{failure:?}-{restart}"));
    let (writer, reader) = (
        Peer::new(&root, 1, retry_delay),
        Peer::new(&root, 2, retry_delay),
    );
    let writer_replica = writer.open_attached().await;
    let mut reader_replica = reader.open_attached().await;

    // the reader stalls on the first entry, so the later ones land on a stream whose commit
    // is already behind and are stored without being committed
    reader.faults.fail(Some(failure));
    for keys in [0..3, 3..5] {
        write(&writer_replica, keys).await;
        wait_until_synced(&reader_replica, &writer_replica).await;
    }
    assert!(rows(&reader_replica).await.is_empty());

    if restart {
        // the backoff is far away: nothing is tried again until the restart
        let attempts = reader.faults.attempts();
        tokio::time::sleep(10 * TICK).await;
        assert_eq!(reader.faults.attempts(), attempts);
        reader.faults.fail(None);
        reader_replica.shutdown().await;
        reader_replica = reader.open().await;
    } else {
        reader.faults.fail(None);
    }
    wait_until_committed(&reader_replica, &writer_replica).await;

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// The clock moves when an op is first tried, so an op that only commits on a retry still has
/// to save it, or a restart would bring back a clock behind an op that was applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retried_commit_saves_the_clock() {
    let root = temp_root("clock");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));
    let writer_replica = writer.open_attached().await;
    let reader_replica = reader.open_attached().await;

    reader.faults.fail(Some(Failure::Prepare));
    write(&writer_replica, 0..1).await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    reader.faults.fail(None);
    wait_until_committed(&reader_replica, &writer_replica).await;

    // the writer's saved clock is its op's timestamp
    assert!(saved_clock(&reader_replica).await > saved_clock(&writer_replica).await);

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A fork that arrives while its parent's commit is behind has to wait for the parent: the
/// entries below the fork point are the parent's to commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fork_of_a_stalled_log_catches_up() {
    let root = temp_root("fork");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db, carries on, and publishes entries 0..5
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;

    reader.faults.fail(Some(Failure::Prepare));
    let reader_replica = reader.open_attached().await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    writer_replica.shutdown().await;

    // the backup writes different entries on top of the first three: a fork at entry 3
    let restored_replica = restored.open_attached().await;
    write(&restored_replica, 5..7).await;
    wait_until("the reader stores the fork", async || {
        let forks_at_3 = |(_, _, fork_size): &(u64, _, Option<u64>)| *fork_size == Some(3);
        heads(&reader_replica)
            .await
            .values()
            .flatten()
            .any(forks_at_3)
    })
    .await;
    assert!(rows(&reader_replica).await.is_empty());

    reader.faults.fail(None);
    wait_until("the reader commits both branches", async || {
        rows(&reader_replica).await.len() == 7
    })
    .await;

    restored_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// How a peer's reducer fails while its commits should stall.
#[derive(Debug, Clone, Copy)]
enum Failure {
    /// `prepare` returns an error.
    Prepare,
    /// The entry's db batch fails when it is committed.
    DbWrite,
}

#[derive(Default)]
struct Faults {
    failure: Mutex<Option<Failure>>,
    attempts: AtomicU64,
}

impl Faults {
    fn fail(&self, failure: Option<Failure>) {
        *self.failure.lock().unwrap() = failure;
    }

    fn failure(&self) -> Option<Failure> {
        *self.failure.lock().unwrap()
    }

    /// How many times an entry's commit was tried.
    fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }
}

struct Peer {
    db_path: PathBuf,
    remote_dir: PathBuf,
    key_seed: u8,
    retry_delay: Duration,
    faults: Arc<Faults>,
}

impl Peer {
    fn new(root: &Path, key_seed: u8, retry_delay: Duration) -> Self {
        Self {
            db_path: root.join(format!("peer-{key_seed}.db")),
            remote_dir: root.join("remote"),
            key_seed,
            retry_delay,
            faults: Default::default(),
        }
    }

    /// The same peer on a copy of its db as it is now (the replica must be shut down).
    fn backup(&self) -> Self {
        let db_path = self.db_path.with_extension("backup.db");
        std::fs::copy(&self.db_path, &db_path).unwrap();
        Self {
            db_path,
            remote_dir: self.remote_dir.clone(),
            faults: Default::default(),
            ..*self
        }
    }

    /// Open this peer's replica and attach the shared remote, which the db remembers across
    /// reopens.
    async fn open_attached(&self) -> Replica<FlakyReducer> {
        let replica = self.open().await;
        replica
            .add_remote(FOLDER, self.remote_dir.to_str().unwrap())
            .await
            .unwrap();
        replica
    }

    /// Open (or reopen) this peer's replica on its db file.
    async fn open(&self) -> Replica<FlakyReducer> {
        let db = SqliteDb::open(&self.db_path).unwrap();
        let (events, _) = event_bus();
        let reducer = FlakyReducer {
            inner: Reducer::new(CONTAINER_ID, "app", &[table_schema()], &db, events)
                .await
                .unwrap(),
            faults: self.faults.clone(),
        };
        let credentials = SoftwareCredentials::new(
            SecretBox::new(Box::new([self.key_seed; 32])),
            SecretBox::new(Box::new([self.key_seed + 100; 32])),
        );
        let mut config = ReplicaConfig {
            pack_sync: PackSyncConfig {
                poll_interval: TICK,
                pack_retry_min: TICK,
                remote_retry_min: TICK,
                ..Default::default()
            },
            commit_retry: CommitRetryConfig {
                poll_interval: TICK,
                retry_min: self.retry_delay,
                retry_max: self.retry_delay,
            },
            ..Default::default()
        };
        config
            .pack_remote_providers
            .insert(FOLDER.into(), Box::new(StdFsRemoteProvider));
        Replica::new(APP_ID, Box::new(db), reducer, Box::new(credentials), config)
            .await
            .unwrap()
    }
}

/// The tables reducer, failing on demand.
struct FlakyReducer {
    inner: Reducer,
    faults: Arc<Faults>,
}

#[async_trait]
impl SqlReducer for FlakyReducer {
    type Op = Op;
    type ReadState = <Reducer as SqlReducer>::ReadState;
    type ApplyState = <Reducer as SqlReducer>::ApplyState;

    fn codec(&self) -> &dyn OpCodec<Op> {
        self.inner.codec()
    }

    async fn prepare(&self, db: &dyn Db, op: &Op) -> Result<Self::ReadState, PrepareError> {
        self.faults.attempts.fetch_add(1, Ordering::Relaxed);
        if let Some(Failure::Prepare) = self.faults.failure() {
            return Err(PrepareError::Internal("injected prepare failure".into()));
        }
        self.inner.prepare(db, op).await
    }

    fn apply(
        &self,
        batch: &mut dyn DbBatch,
        timestamp: Timestamp,
        op: &Op,
        read: Self::ReadState,
    ) -> Result<Self::ApplyState, ApplyError> {
        let apply_state = self.inner.apply(batch, timestamp, op, read)?;
        if let Some(Failure::DbWrite) = self.faults.failure() {
            batch.add_statement(r#"INSERT INTO "no_such_table" VALUES (1)"#, &[]);
        }
        Ok(apply_state)
    }

    fn post_apply(&self, apply_state: Self::ApplyState, batch_result: &[DbStatementResult]) {
        self.inner.post_apply(apply_state, batch_result)
    }
}

fn table_schema() -> TableSchema {
    TableSchema::new(
        TABLE,
        "t".into(),
        vec!["k0".into()],
        vec![ColumnSchema {
            name: "v".into(),
            id: COL_V,
        }],
    )
    .unwrap()
}

fn upsert(key: i64) -> Op {
    Op::Upsert(Upsert {
        table_id: TABLE,
        primary_key: vec![Value::I64(key)],
        sets: vec![ColumnSet {
            column_id: COL_V,
            value: Value::Text(format!("v{key}")),
        }],
        nulls: vec![],
    })
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ubiquisync-commit-retry-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn write(replica: &Replica<FlakyReducer>, keys: Range<i64>) {
    for key in keys {
        replica.exec(None, upsert(key)).await.unwrap();
    }
}

/// Every stream head the replica has stored, comparable across replicas.
async fn heads(replica: &Replica<FlakyReducer>) -> HashMap<(PeerId, ContainerId), StreamHeads> {
    replica.sync_snapshot().await.unwrap().streams
}

/// The table rows the replica has committed.
async fn rows(replica: &Replica<FlakyReducer>) -> Vec<Vec<DbValue>> {
    replica
        .query(r#"SELECT "k0", "v" FROM "t" ORDER BY "k0""#, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.values)
        .collect()
}

/// The replica's saved clock, if it has saved one.
async fn saved_clock(replica: &Replica<FlakyReducer>) -> Option<i64> {
    let rows = replica
        .query(r#"SELECT "timestamp" FROM "__replica_hlc""#, &[])
        .await
        .unwrap();
    match rows.first()?.values[0] {
        DbValue::Integer(clock) => Some(clock),
        ref other => panic!("unexpected clock value: {other:?}"),
    }
}

async fn wait_until_synced(reader: &Replica<FlakyReducer>, writer: &Replica<FlakyReducer>) {
    wait_until("the reader stores the writer's entries", async || {
        heads(reader).await == heads(writer).await
    })
    .await;
}

async fn wait_until_committed(reader: &Replica<FlakyReducer>, writer: &Replica<FlakyReducer>) {
    wait_until("the reader commits the stored entries", async || {
        rows(reader).await == rows(writer).await
    })
    .await;
}

async fn wait_until<F: Future<Output = bool>>(what: &str, mut done: impl FnMut() -> F) {
    let started = Instant::now();
    while !done().await {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting until {what}"
        );
        tokio::time::sleep(TICK).await;
    }
}
