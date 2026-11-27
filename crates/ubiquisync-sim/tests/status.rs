//! Sync status and events between two replicas on a shared folder: `status()` levels and
//! counts, a stalled log, pause and resume, and what `watch()` delivers.

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
use futures::StreamExt;
use secrecy::SecretBox;
use ubiquisync_core::{
    crypto::credentials::software::SoftwareCredentials,
    event::event_bus,
    hlc::Timestamp,
    ids::{AppId, ContainerId, PeerId},
    pack::StdFsRemoteProvider,
};
use ubiquisync_sql::{
    Exec,
    db::{Db, DbBatch, DbStatementResult},
    op::OpCodec,
    reducer::{ApplyError, PrepareError, Reducer as SqlReducer},
    replica::{
        CommitRetryConfig, Level, PackSyncConfig, Reach, Replica, ReplicaConfig, ReplicaDeps,
        Stall, SyncEvent, SyncEventKind, SyncEvents, SyncStatus, TaskState,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn up_to_date_after_converging() {
    let root = temp_root("converge");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open().await, b.open().await);
    assert_eq!(ra.status().await.unwrap().level, Level::NoRemote);
    a.attach(&ra).await;
    b.attach(&rb).await;

    write(&ra, 0..3).await;
    write(&rb, 3..5).await;
    for r in [&ra, &rb] {
        wait_until("both replicas are up to date with both logs", async || {
            let s = r.status().await.unwrap();
            // a file is counted as read once its whole round is saved, just after its
            // entries are applied
            s.level == Level::UpToDate
                && s.logs.iter().map(|l| l.applied).sum::<u64>() == 5
                && s.remotes[0].files_read >= 2
        })
        .await;
    }

    let (sa, sb) = (ra.status().await.unwrap(), rb.status().await.unwrap());
    for s in [&sa, &sb] {
        assert_eq!((s.to_send, s.to_apply), (0, 0));
        assert!(s.logs[0].is_this_device);
        assert_eq!(s.remotes.len(), 1);
        assert_eq!(s.remotes[0].reach, Reach::Ok);
        assert!(s.remotes[0].last_read_at.is_some());
        assert!(s.remotes[0].last_publish_at.is_some());
        assert!(s.tasks.iter().all(|t| t.state == TaskState::Running));
        assert_eq!(s.tasks.len(), 2);
    }
    let log_of = |s: &SyncStatus, device: PeerId| {
        let l = s.logs.iter().find(|l| l.device == device).unwrap();
        (l.received, l.applied, l.in_every_remote)
    };
    assert_eq!(log_of(&sa, sa.this_device), (3, 3, 3));
    assert_eq!(log_of(&sa, sb.this_device), (2, 2, 2));
    assert_eq!(log_of(&sb, sa.this_device), (3, 3, 3));

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paused_device_counts_what_it_has_to_send() {
    let root = temp_root("pause");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open_attached().await, b.open_attached().await);

    ra.set_sync_paused(true);
    write(&ra, 0..4).await;
    // a few rounds pass and nothing leaves the paused device
    tokio::time::sleep(10 * TICK).await;
    let s = ra.status().await.unwrap();
    assert_eq!(s.level, Level::Paused);
    assert!(s.paused);
    assert_eq!(s.to_send, 4);
    assert_eq!(s.logs[0].in_every_remote, 0);
    assert!(rb.status().await.unwrap().logs.is_empty());

    ra.set_sync_paused(false);
    wait_until("the resumed device has sent everything", async || {
        ra.status().await.unwrap().level == Level::UpToDate
    })
    .await;
    wait_until("the other device applied it", async || {
        let s = rb.status().await.unwrap();
        s.level == Level::UpToDate && s.logs.iter().any(|l| l.applied == 4)
    })
    .await;

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_log_shows_its_stall_and_recovers() {
    let root = temp_root("stall");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open_attached().await, b.open_attached().await);
    let mut events = rb.watch();

    b.fail.store(true, Ordering::Relaxed);
    write(&ra, 0..3).await;
    let device_a = ra.status().await.unwrap().this_device;
    wait_until(
        "the reader stores the entries but cannot apply them",
        async || {
            let s = rb.status().await.unwrap();
            s.to_apply == 3
        },
    )
    .await;
    let s = rb.status().await.unwrap();
    assert_eq!(s.level, Level::Attention);
    let log = s.logs.iter().find(|l| l.device == device_a).unwrap();
    assert_eq!((log.received, log.applied), (3, 0));
    assert!(
        matches!(log.stall, Some(Stall::Retrying { attempts, .. }) if attempts >= 1),
        "unexpected stall {:?}",
        log.stall
    );

    b.fail.store(false, Ordering::Relaxed);
    wait_until("the reader applies the entries", async || {
        let s = rb.status().await.unwrap();
        s.to_apply == 0 && s.level == Level::UpToDate
    })
    .await;

    let seen = collect_until(
        &mut events,
        |e| matches!(e.kind, SyncEventKind::Recovered { device, .. } if device == device_a),
    )
    .await;
    assert!(seen.iter().any(|e| matches!(
        &e.kind,
        SyncEventKind::Stalled { device, stall: Stall::Retrying { .. }, .. } if *device == device_a
    )));

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watch_reports_a_simple_exchange() {
    let root = temp_root("watch");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open_attached().await, b.open_attached().await);
    let (mut ea, mut eb) = (ra.watch(), rb.watch());
    let device_a = ra.status().await.unwrap().this_device;

    write(&ra, 0..2).await;

    let seen_a = collect_until(&mut ea, |e| {
        matches!(e.kind, SyncEventKind::FilePublished { .. })
    })
    .await;
    assert!(seen_a.iter().any(|e| matches!(
        e.kind,
        SyncEventKind::Applied { device, from: 1, to: 2, .. } if device == device_a
    )));
    let SyncEventKind::FilePublished { entries, bytes, .. } = seen_a.last().unwrap().kind else {
        unreachable!()
    };
    assert!(entries >= 1 && bytes > 0);

    // entries are applied while their file is read, so `FileRead` comes after `Applied`
    let seen_b = collect_until_all(&mut eb, |seen| {
        let applied = seen.iter().position(|e| {
            matches!(e.kind, SyncEventKind::Applied { device, to: 2, .. } if device == device_a)
        });
        applied.is_some_and(|i| {
            seen[i..].iter().any(|e| {
                matches!(e.kind, SyncEventKind::FileRead { publisher, .. } if publisher == device_a)
            })
        })
    })
    .await;
    assert!(seen_b.iter().any(|e| matches!(
        e.kind,
        SyncEventKind::DeviceSeen { device } if device == device_a
    )));

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lagging_subscriber_gets_missed() {
    let root = temp_root("missed");
    let a = Peer::new(&root, 1);
    let ra = a.open().await;
    let mut events = ra.watch();
    // every local write sends one `Applied`; the channel holds 1024
    write(&ra, 0..1100).await;
    let first = tokio::time::timeout(Duration::from_secs(5), futures_next(&mut events))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(first.kind, SyncEventKind::Missed(n) if n >= 76),
        "expected Missed first, got {first:?}"
    );
    // then the stream carries on
    let next = futures_next(&mut events).await.unwrap();
    assert!(matches!(next.kind, SyncEventKind::Applied { .. }));

    ra.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_task_is_reported_and_restarted() {
    let root = temp_root("panic");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open_attached().await, b.open_attached().await);
    let mut events = rb.watch();

    b.panic.store(true, Ordering::Relaxed);
    write(&ra, 0..1).await;
    collect_until(&mut events, |e| {
        matches!(e.kind, SyncEventKind::TaskStopped { .. })
    })
    .await;
    let s = rb.status().await.unwrap();
    assert_eq!(s.level, Level::Attention);
    assert!(s.tasks.iter().any(|t| matches!(
        &t.state,
        TaskState::Stopped { error } if error.contains("injected panic")
    )));

    b.panic.store(false, Ordering::Relaxed);
    // restarted after a second
    wait_until("the reader applies the entry", async || {
        let s = rb.status().await.unwrap();
        s.level == Level::UpToDate && s.to_apply == 0 && s.logs.len() == 1
    })
    .await;
    let s = rb.status().await.unwrap();
    assert!(s.tasks.iter().all(|t| t.state == TaskState::Running));
    assert!(s.tasks.iter().any(|t| t.restarts >= 1));

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(feature = "inspect")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inspect_reads_entries_of_another_device() {
    use ubiquisync_core::ids::LogId;
    use ubiquisync_sql::replica::EntryKind;
    use ubiquisync_tables::codec::decode_any;

    let root = temp_root("inspect");
    let (a, b) = (Peer::new(&root, 1), Peer::new(&root, 2));
    let (ra, rb) = (a.open_attached().await, b.open_attached().await);
    write(&ra, 0..3).await;
    let device_a = ra.status().await.unwrap().this_device;
    wait_until("the reader applies the entries", async || {
        rb.status()
            .await
            .unwrap()
            .logs
            .iter()
            .any(|l| l.applied == 3)
    })
    .await;

    let devices = rb.devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(devices[0].is_this_device && !devices[1].is_this_device);
    assert_eq!(devices[1].device, device_a);

    let log = LogId {
        peer_id: device_a,
        container_id: CONTAINER_ID,
    };
    let entries = rb.entries(log, 1..10).await.unwrap();
    assert_eq!(entries.iter().map(|e| e.index).collect::<Vec<_>>(), [1, 2]);
    for (e, key) in entries.iter().zip(1..) {
        assert_eq!(e.kind, EntryKind::Op);
        assert!(e.applied && e.fork.is_none() && e.at.is_some());
        let view = decode_any(e.op.as_deref().unwrap()).unwrap();
        assert_eq!(view.table, TABLE);
        assert_eq!(view.key, vec![Value::I64(key)]);
        assert_eq!(
            view.sets,
            vec![(COL_V, Some(Value::Text(format!("v{key}"))))]
        );
    }
    // the reader's copy matches the author's
    assert_eq!(ra.entries(log, 1..10).await.unwrap(), entries);

    let remote = rb.status().await.unwrap().remotes[0].id;
    let files = rb.remote_files(remote).await.unwrap();
    assert!(
        files
            .iter()
            .any(|f| f.publisher == device_a && f.read && f.header_ok)
    );

    ra.shutdown().await;
    rb.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Collects events until one matches `done` (included).
async fn collect_until(
    events: &mut SyncEvents,
    done: impl Fn(&SyncEvent) -> bool,
) -> Vec<SyncEvent> {
    collect_until_all(events, |seen| seen.last().is_some_and(&done)).await
}

async fn futures_next(events: &mut SyncEvents) -> Option<SyncEvent> {
    events.next().await
}

/// Collects events until `done` holds for the events seen so far, failing after 10 s.
async fn collect_until_all(
    events: &mut SyncEvents,
    done: impl Fn(&[SyncEvent]) -> bool,
) -> Vec<SyncEvent> {
    let mut seen = vec![];
    let started = Instant::now();
    loop {
        let left = Duration::from_secs(10).saturating_sub(started.elapsed());
        let ev = tokio::time::timeout(left, futures_next(events))
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for an event, saw {seen:#?}"))
            .expect("event stream ended");
        seen.push(ev);
        if done(&seen) {
            return seen;
        }
    }
}

struct Peer {
    db_path: PathBuf,
    remote_dir: PathBuf,
    key_seed: u8,
    fail: Arc<AtomicBool>,
    panic: Arc<AtomicBool>,
}

impl Peer {
    fn new(root: &Path, key_seed: u8) -> Self {
        Self {
            db_path: root.join(format!("peer-{key_seed}.db")),
            remote_dir: root.join("remote"),
            key_seed,
            fail: Default::default(),
            panic: Default::default(),
        }
    }

    async fn open_attached(&self) -> Replica<FlakyReducer> {
        let replica = self.open().await;
        self.attach(&replica).await;
        replica
    }

    async fn attach(&self, replica: &Replica<FlakyReducer>) {
        replica
            .add_remote(FOLDER, self.remote_dir.to_str().unwrap())
            .await
            .unwrap();
    }

    async fn open(&self) -> Replica<FlakyReducer> {
        let db = SqliteDb::open(&self.db_path).unwrap();
        let (events, _) = event_bus();
        let reducer = FlakyReducer {
            inner: Reducer::new(CONTAINER_ID, "app", &[table_schema()], &db, events)
                .await
                .unwrap(),
            fail: self.fail.clone(),
            panic: self.panic.clone(),
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
                retry_min: TICK,
                retry_max: TICK,
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

/// The tables reducer, whose `prepare` fails while `fail` is set and panics while `panic`
/// is set.
struct FlakyReducer {
    inner: Reducer,
    fail: Arc<AtomicBool>,
    panic: Arc<AtomicBool>,
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
        if self.panic.load(Ordering::Relaxed) {
            panic!("injected panic");
        }
        if self.fail.load(Ordering::Relaxed) {
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

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ubiquisync-status-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

async fn write(replica: &Replica<FlakyReducer>, keys: std::ops::Range<i64>) {
    for key in keys {
        let op = Op::Upsert(Upsert {
            table_id: TABLE,
            primary_key: vec![Value::I64(key)],
            sets: vec![ColumnSet {
                column_id: COL_V,
                value: Value::Text(format!("v{key}")),
            }],
            nulls: vec![],
        });
        replica.exec(None, op).await.unwrap();
    }
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
