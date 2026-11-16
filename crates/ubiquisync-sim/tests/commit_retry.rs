//! A replica whose commits stall keeps storing what it syncs, and the commit retry loop
//! applies it once the cause is gone.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
    ids::{AppId, ContainerId},
    pack::StdFsRemoteProvider,
};
use ubiquisync_sql::{
    Exec, SqlQueryStore,
    db::{Db, DbBatch, DbStatementResult, DbValue},
    op::OpCodec,
    reducer::{ApplyError, PrepareError, Reducer as SqlReducer},
    replica::{CommitRetryConfig, PackSyncConfig, Replica, ReplicaConfig},
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

#[test_case(TICK, false ; "once the backoff passes")]
#[test_case(Duration::from_hours(1), true ; "right after a restart, whatever the backoff")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_commits_catch_up(retry_delay: Duration, restart: bool) {
    let root = std::env::temp_dir().join(format!("ubiquisync-commit-retry-{restart}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let writer = Peer::new(&root, 1, retry_delay);
    let reader = Peer::new(&root, 2, retry_delay);
    let writer_replica = writer.open().await;
    let mut reader_replica = reader.open().await;

    // the reader stalls on the first entry, so the later ones land on a stream whose commit
    // is already behind and are stored without being committed
    reader.reducer_down.store(true, Ordering::Relaxed);
    for batch in [0..3, 3..5] {
        for key in batch {
            writer_replica.exec(None, upsert(key)).await.unwrap();
        }
        wait_until("the reader stores the writer's entries", async || {
            heads(&reader_replica).await == heads(&writer_replica).await
        })
        .await;
    }
    assert!(rows(&reader_replica).await.is_empty());

    reader.reducer_down.store(false, Ordering::Relaxed);
    if restart {
        // the backoff is far away, so only the restart can retry the commit
        tokio::time::sleep(10 * TICK).await;
        assert!(rows(&reader_replica).await.is_empty());
        reader_replica.shutdown().await;
        reader_replica = reader.open().await;
    }
    wait_until("the reader commits the stored entries", async || {
        rows(&reader_replica).await == rows(&writer_replica).await
    })
    .await;

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

struct Peer {
    db_path: PathBuf,
    remote_dir: PathBuf,
    key_seed: u8,
    retry_delay: Duration,
    reducer_down: Arc<AtomicBool>,
}

impl Peer {
    fn new(root: &Path, key_seed: u8, retry_delay: Duration) -> Self {
        Self {
            db_path: root.join(format!("peer-{key_seed}.db")),
            remote_dir: root.join("remote"),
            key_seed,
            retry_delay,
            reducer_down: Default::default(),
        }
    }

    /// Open (or reopen) this peer's replica on its db file, attached to the shared remote.
    async fn open(&self) -> Replica<FlakyReducer> {
        let first_open = !self.db_path.exists();
        let db = SqliteDb::open(&self.db_path).unwrap();
        let (events, _) = event_bus();
        let reducer = FlakyReducer {
            inner: Reducer::new(CONTAINER_ID, "app", &[table_schema()], &db, events)
                .await
                .unwrap(),
            down: self.reducer_down.clone(),
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
        let replica = Replica::new(APP_ID, Box::new(db), reducer, Box::new(credentials), config)
            .await
            .unwrap();
        if first_open {
            replica
                .add_remote(FOLDER, self.remote_dir.to_str().unwrap())
                .await
                .unwrap();
        }
        replica
    }
}

/// The tables reducer, with a switch that makes every `prepare` fail the way a broken
/// database would.
struct FlakyReducer {
    inner: Reducer,
    down: Arc<AtomicBool>,
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
        if self.down.load(Ordering::Relaxed) {
            return Err(PrepareError::Internal("reducer is down".into()));
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
        self.inner.apply(batch, timestamp, op, read)
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

/// Every stream head the replica has stored, comparable across replicas.
async fn heads(replica: &Replica<FlakyReducer>) -> impl PartialEq + use<> {
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
