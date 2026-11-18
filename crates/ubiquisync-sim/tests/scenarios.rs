//! Scripted sync scenarios between a few replicas on a shared folder remote: stalled commits
//! that the commit retry loop has to finish, and forked logs. (`sim.rs` is the randomized
//! counterpart.)

use std::{
    collections::HashMap,
    future::Future,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
        CommitRetryConfig, PackSyncConfig, Replica, ReplicaConfig, ReplicaDeps,
        test_support::BranchHeads,
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

    // the reader stalls on the first entry, so the later ones land on a branch whose commit
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
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..7).await;
    restored.attach(&restored_replica).await;
    wait_until_forked_at(&reader_replica, 3).await;
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

/// A device restored from a backup forks its own log as soon as it writes and syncs. Both
/// copies keep writing to their own lineage, each gets the other's entries as a fork, and every
/// device ends up with the same rows.
#[test_case(3 ; "backup taken after three entries")]
#[test_case(0 ; "backup taken before the log had any entry")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forked_own_log_keeps_writing(backed_up: i64) {
    let root = temp_root(&format!("own-fork-{backed_up}"));
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db, carries on, and publishes entries 0..5
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..backed_up).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, backed_up..5).await;
    let reader_replica = reader.open_attached().await;

    // the backup writes before it syncs, so what the original wrote since is a fork of its log
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..7).await;
    restored.attach(&restored_replica).await;
    for replica in [&restored_replica, &writer_replica, &reader_replica] {
        wait_until_forked_at(replica, backed_up as u64).await;
    }

    // both copies of the forked log can still write
    write(&restored_replica, 7..9).await;
    write(&writer_replica, 9..11).await;
    for replica in [&restored_replica, &writer_replica, &reader_replica] {
        wait_until("every device has every row", async || {
            rows(replica).await.len() == 11
        })
        .await;
    }
    assert_eq!(rows(&restored_replica).await, rows(&reader_replica).await);
    assert_eq!(rows(&writer_replica).await, rows(&reader_replica).await);

    for replica in [restored_replica, writer_replica, reader_replica] {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// `changed_at` is a replica's own record of when each of its branches last changed. Local
/// writes and ingested segments both move a branch's stamp past every stamp the replica handed
/// out before. A branch whose head didn't move keeps its stamp, also when its stored entries
/// are committed later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_follows_branch_heads() {
    let root = temp_root("changed-at");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));
    let started = wall_ms();
    let writer_replica = writer.open_attached().await;
    let writer_id = writer_replica.sync_snapshot().await.unwrap().self_peer;

    // every local write moves the stamp of the log it wrote to
    write(&writer_replica, 0..1).await;
    let first = changed_at(&writer_replica).await[&writer_id];
    assert!(first >= started);
    write(&writer_replica, 1..2).await;
    let second = changed_at(&writer_replica).await[&writer_id];
    assert!(second > first);

    // the reader stamps the writer's log when it stores it, not when it was written
    let reader_opened = wall_ms();
    let reader_replica = reader.open_attached().await;
    let reader_id = reader_replica.sync_snapshot().await.unwrap().self_peer;
    wait_until_synced(&reader_replica, &writer_replica).await;
    let ingested = changed_at(&reader_replica).await[&writer_id];
    assert!(ingested >= reader_opened);

    // the reader's own write is stamped after that, and leaves the writer's log alone
    write(&reader_replica, 10..11).await;
    let stamps = changed_at(&reader_replica).await;
    assert_eq!(stamps[&writer_id], ingested);
    assert!(stamps[&reader_id] > ingested);

    // more from the writer moves its log past everything else on the reader, here stored
    // without being committed
    reader.faults.fail(Some(Failure::Prepare));
    write(&writer_replica, 2..3).await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    let stamps = changed_at(&reader_replica).await;
    assert!(stamps[&writer_id] > stamps[&reader_id]);

    // committing the stored entry later doesn't move the head, so the stamp stays
    reader.faults.fail(None);
    wait_until("the reader commits the stored entry", async || {
        rows(&reader_replica).await.len() == 4
    })
    .await;
    assert_eq!(changed_at(&reader_replica).await, stamps);

    writer_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A fork is a new branch row, stamped when it is first stored, and the branch it forked from
/// keeps its stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_stamps_a_new_fork() {
    let root = temp_root("changed-at-fork");
    let (writer, reader) = (Peer::new(&root, 1, TICK), Peer::new(&root, 2, TICK));

    // the writer backs up its db after three entries, carries on to five, and the reader syncs
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;
    let reader_replica = reader.open_attached().await;
    wait_until_synced(&reader_replica, &writer_replica).await;
    writer_replica.shutdown().await;
    let before = reader_replica.branch_changes().await.unwrap();
    assert_eq!(before.len(), 1);
    let (_, _, _, root_stamp) = before[0];

    // the backup writes three entries of its own on top of the first three: a fork with head 6
    let restored_replica = restored.open().await;
    write(&restored_replica, 5..8).await;
    restored.attach(&restored_replica).await;
    wait_until("the reader stores the whole fork", async || {
        let changes = reader_replica.branch_changes().await.unwrap();
        changes.iter().any(|(_, _, head_size, _)| *head_size == 6)
    })
    .await;

    // ordered by stamp: the branch the reader already had, then the fork
    let after = reader_replica.branch_changes().await.unwrap();
    let stamps: Vec<(u64, u64)> = after.iter().map(|(_, _, size, at)| (*size, *at)).collect();
    assert_eq!(stamps.len(), 2);
    assert_eq!(stamps[0], (5, root_stamp));
    assert_eq!(stamps[1].0, 6);
    assert!(stamps[1].1 > root_stamp);

    restored_replica.shutdown().await;
    reader_replica.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// A local write to a log whose commit is stalled is stored without being committed, and
/// still moves the stamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_at_moves_on_a_write_to_a_stalled_log() {
    let root = temp_root("changed-at-stalled");
    let writer = Peer::new(&root, 1, Duration::from_hours(1));

    // a restored copy of the writer gets the entries the original wrote after the backup; they
    // extend its own log, and its commit of them stalls
    let writer_replica = writer.open().await;
    write(&writer_replica, 0..3).await;
    writer_replica.shutdown().await;
    let restored = writer.backup();
    let writer_replica = writer.open_attached().await;
    write(&writer_replica, 3..5).await;
    restored.faults.fail(Some(Failure::Prepare));
    let restored_replica = restored.open_attached().await;
    wait_until_synced(&restored_replica, &writer_replica).await;
    writer_replica.shutdown().await;
    assert_eq!(rows(&restored_replica).await.len(), 3);
    let writer_id = restored_replica.sync_snapshot().await.unwrap().self_peer;
    let stalled = changed_at(&restored_replica).await[&writer_id];

    // the write goes on top of the stalled entries
    write(&restored_replica, 5..6).await;
    assert_eq!(rows(&restored_replica).await.len(), 3);
    assert!(changed_at(&restored_replica).await[&writer_id] > stalled);

    restored_replica.shutdown().await;
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

    async fn open_attached(&self) -> Replica<FlakyReducer> {
        let replica = self.open().await;
        self.attach(&replica).await;
        replica
    }

    /// Attach the shared remote, which the db remembers across reopens.
    async fn attach(&self, replica: &Replica<FlakyReducer>) {
        replica
            .add_remote(FOLDER, self.remote_dir.to_str().unwrap())
            .await
            .unwrap();
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
        let config = ReplicaConfig {
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
        };
        let deps = ReplicaDeps {
            db: Box::new(db),
            reducer,
            credentials: Box::new(credentials),
            pack_remote_providers: [(FOLDER.into(), Box::new(StdFsRemoteProvider) as _)].into(),
        };
        Replica::new(APP_ID, deps, config).await.unwrap()
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

/// Every branch head the replica has stored, comparable across replicas.
async fn heads(replica: &Replica<FlakyReducer>) -> HashMap<(PeerId, ContainerId), BranchHeads> {
    replica.sync_snapshot().await.unwrap().branches
}

/// The replica's `changed_at` stamp for each author's log. Only for scenarios where every
/// peer has a single branch (one container, no forks).
async fn changed_at(replica: &Replica<FlakyReducer>) -> HashMap<PeerId, u64> {
    let changes = replica.branch_changes().await.unwrap();
    let stamps: HashMap<PeerId, u64> = changes
        .iter()
        .map(|(peer, _, _, changed_at)| (*peer, *changed_at))
        .collect();
    assert_eq!(
        stamps.len(),
        changes.len(),
        "a peer has more than one branch"
    );
    stamps
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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

/// Waits until the replica holds a branch that forked off its parent at `fork_size`.
async fn wait_until_forked_at(replica: &Replica<FlakyReducer>, fork_size: u64) {
    wait_until("the replica stores the fork", async || {
        let heads = heads(replica).await;
        heads
            .values()
            .flatten()
            .any(|head| head.2 == Some(fork_size))
    })
    .await;
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
