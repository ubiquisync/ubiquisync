//! Multi-replica pack sync simulation (packs-and-topics.md §2.17).
//!
//! Several replicas, each on its own SQLite file, write conflicting rows and
//! sync through temp-dir folder remotes while a fault-injecting remote wrapper
//! fails, truncates and restarts things. Then writes stop, faults turn off, and
//! every group of peers linked through shared remotes must converge to the same
//! stream heads and table rows with nothing blocked or left to publish.
//!
//! Replay a run with `SIM_SEED=<seed>`; the seed is printed at the start.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use secrecy::SecretBox;
use ubiquisync_core::{
    crypto::credentials::software::SoftwareCredentials,
    event::event_bus,
    ids::{AppId, ContainerId},
    pack::{DirEntry, FileRemote, FileRemoteError, FileRemoteProvider, StdFsRemoteProvider},
};
use ubiquisync_sql::{
    Exec, SqlQueryStore,
    db::DbValue,
    replica::{PackSyncConfig, Replica, ReplicaConfig, test_support::SyncSnapshot},
};
use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::{
    col_type::ColType,
    id::{ColumnId, TableId},
    op::{ColumnSet, Delete, Op, Upsert, Value},
    reducer::Reducer,
    schema::{ColumnSchema, TableSchema},
};

const APP_ID: AppId = AppId([7; 16]);
const CONTAINER_ID: ContainerId = ContainerId([9; 16]);
const TABLE: TableId = TableId::new(&[ColType::I64], 1);
const COL_V: ColumnId = ColumnId::new(0, ColType::Text);
const FAULTY: &str = "faulty";

// SQLite and the folder remote do blocking IO (including fsync) on the async workers, and
// tokio only services timers between task polls, so with few workers a peer's poll timer
// can fire seconds late. Plenty of workers keeps settle times short.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
async fn sim_short() {
    run(SimParams {
        write_for: Duration::from_secs(3),
        ..SimParams::default()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "long run"]
async fn sim_long() {
    run(SimParams {
        write_for: Duration::from_secs(90),
        ..SimParams::default()
    })
    .await;
}

/// Many peers writing fast, with bursts. Prints write throughput, sync round timings
/// and remote/db sizes. Run with `--ignored sim_load --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "load run"]
async fn sim_load() {
    run(SimParams {
        write_for: Duration::from_secs(60),
        settle_timeout: Duration::from_secs(300),
        peers: (12, 12),
        remotes: (2, 3),
        keys: 1000,
        write_gap_ms: (1, 10),
        p_burst: 0.01,
        burst: (50, 200),
        p_error: 0.01,
        p_truncate: 0.005,
        p_restart: 0.0005,
    })
    .await;
}

struct SimParams {
    write_for: Duration,
    settle_timeout: Duration,
    peers: (u64, u64),
    remotes: (u64, u64),
    keys: i64,
    /// Pause between a peer's writes, in ms (`lo..hi`).
    write_gap_ms: (u64, u64),
    /// Chance per write of starting a burst of back-to-back writes.
    p_burst: f64,
    burst: (u64, u64),
    /// Chance per remote call of a transient error.
    p_error: f64,
    /// Chance per remote read of returning a truncated file.
    p_truncate: f64,
    /// Chance per write of a peer restarting (shutdown and reopen its db).
    p_restart: f64,
}

impl Default for SimParams {
    fn default() -> Self {
        Self {
            write_for: Duration::from_secs(3),
            settle_timeout: Duration::from_secs(30),
            peers: (3, 6),
            remotes: (1, 3),
            keys: 20,
            write_gap_ms: (5, 50),
            p_burst: 0.0,
            burst: (1, 2),
            p_error: 0.05,
            p_truncate: 0.02,
            p_restart: 0.005,
        }
    }
}

async fn run(params: SimParams) {
    let seed = std::env::var("SIM_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
        });
    eprintln!("SIM_SEED={seed}");
    timing::init();
    let mut rng = Rng(seed);

    let root = std::env::temp_dir().join(format!("ubiquisync-sim-{seed}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let faults = Arc::new(Faults {
        on: AtomicBool::new(true),
        rng: Mutex::new(Rng(rng.next())),
        p_error: params.p_error,
        p_truncate: params.p_truncate,
        calls: (0..params.peers.1).map(|_| AtomicU64::new(0)).collect(),
    });

    // topology
    let n_peers = rng.range(params.peers.0, params.peers.1 + 1) as usize;
    let n_remotes = rng.range(params.remotes.0, params.remotes.1 + 1) as usize;
    let remote_dirs: Vec<PathBuf> = (0..n_remotes)
        .map(|i| root.join(format!("remote-{i}")))
        .collect();
    let mut attached: Vec<Vec<usize>> = vec![];
    for _ in 0..n_peers {
        let mut set: Vec<usize> = (0..n_remotes).filter(|_| rng.chance(0.5)).collect();
        if set.is_empty() {
            set.push(rng.range(0, n_remotes as u64) as usize);
        }
        attached.push(set);
    }
    let groups = connected_groups(&attached);
    eprintln!("peers={n_peers} remotes={n_remotes} attached={attached:?} groups={groups:?}");

    // start peers
    let mut peers = vec![];
    for (i, remotes) in attached.iter().enumerate() {
        let peer = Peer {
            index: i,
            db_path: root.join(format!("peer-{i}.db")),
            keys: (rng.bytes(), rng.bytes()),
            faults: faults.clone(),
        };
        let replica = peer.open().await;
        for r in remotes {
            replica
                .add_remote(FAULTY, remote_dirs[*r].to_str().unwrap())
                .await
                .unwrap();
        }
        peers.push((peer, replica));
    }

    // write phase: one task per peer, each owning its replica so it can restart it
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = vec![];
    for (peer, replica) in peers {
        let stop = stop.clone();
        let mut rng = Rng(rng.next());
        let keys = params.keys;
        let p_restart = params.p_restart;
        let (gap, p_burst, burst) = (params.write_gap_ms, params.p_burst, params.burst);
        tasks.push(tokio::spawn(async move {
            let mut replica = replica;
            let mut writes = 0u64;
            let mut restarts = 0u64;
            let mut burst_left = 0;
            while !stop.load(Ordering::Relaxed) {
                if burst_left > 0 {
                    burst_left -= 1;
                } else {
                    tokio::time::sleep(Duration::from_millis(rng.range(gap.0, gap.1))).await;
                    if rng.chance(p_burst) {
                        burst_left = rng.range(burst.0, burst.1);
                    }
                }
                let key = Value::I64(rng.range(0, keys as u64) as i64);
                let op = if rng.chance(0.2) {
                    Op::Delete(Delete {
                        table_id: TABLE,
                        primary_key: vec![key],
                    })
                } else {
                    Op::Upsert(Upsert {
                        table_id: TABLE,
                        primary_key: vec![key],
                        sets: vec![ColumnSet {
                            column_id: COL_V,
                            value: Value::Text(format!("p{}-{}", peer.index, writes)),
                        }],
                        nulls: vec![],
                    })
                };
                replica.exec(None, op).await.unwrap();
                writes += 1;
                if rng.chance(p_restart) {
                    replica.shutdown().await;
                    replica = peer.open().await;
                    restarts += 1;
                }
            }
            eprintln!("peer {}: {writes} writes, {restarts} restarts", peer.index);
            (peer, replica)
        }));
    }
    timing::reset();
    let write_started = Instant::now();
    tokio::time::sleep(params.write_for).await;
    stop.store(true, Ordering::Relaxed);
    faults.on.store(false, Ordering::Relaxed);
    let mut peers = vec![];
    for t in tasks {
        peers.push(t.await.unwrap());
    }
    let write_elapsed = write_started.elapsed();
    let write_timings = timing::take();

    // settle: keep syncing until every group agrees and is quiescent for a few polls
    let started = Instant::now();
    let mut stable = 0;
    let mut polls = 0u64;
    let mut last_report = String::new();
    while stable < 3 {
        if started.elapsed() > params.settle_timeout {
            panic!(
                "did not converge within {:?} (SIM_SEED={seed})\n{last_report}",
                params.settle_timeout
            );
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut states = vec![];
        for (_, replica) in &peers {
            states.push(PeerState::read(replica).await);
        }
        match check_converged(&groups, &states) {
            Ok(()) => stable += 1,
            Err(report) => {
                stable = 0;
                last_report = report;
            }
        }
        if polls.is_multiple_of(5) && stable == 0 {
            eprintln!(
                "settling {:?}: remote calls per peer {:?}\n{}{last_report}",
                started.elapsed(),
                faults
                    .calls
                    .iter()
                    .map(|c| c.load(Ordering::Relaxed))
                    .collect::<Vec<_>>(),
                head_matrix(&states)
            );
        }
        polls += 1;
    }
    eprintln!("converged after {:?}", started.elapsed());
    let settle_timings = timing::take();

    // summary
    let mut total_writes = 0;
    for (_, replica) in &peers {
        let snap = replica.sync_snapshot().await.unwrap();
        total_writes = snap
            .streams
            .values()
            .map(|heads| heads.iter().map(|(size, _, _)| *size).max().unwrap_or(0))
            .sum::<u64>()
            .max(total_writes);
    }
    eprintln!(
        "\n== summary: {} peers, {} remotes, {total_writes} entries written in {write_elapsed:.1?} ({:.0}/s)",
        peers.len(),
        remote_dirs.len(),
        total_writes as f64 / write_elapsed.as_secs_f64()
    );
    eprintln!(
        "-- span timings while writing (wall time per span):\n{}",
        timing::report(&write_timings)
    );
    eprintln!(
        "-- span timings while settling:\n{}",
        timing::report(&settle_timings)
    );
    for (i, dir) in remote_dirs.iter().enumerate() {
        let (files, bytes) = dir_stats(dir);
        eprintln!(
            "-- remote {i}: {files} files, {:.1} MiB",
            bytes as f64 / (1 << 20) as f64
        );
    }
    for (peer, _) in &peers {
        let bytes: u64 = ["", "-wal"]
            .iter()
            .filter_map(|ext| std::fs::metadata(format!("{}{ext}", peer.db_path.display())).ok())
            .map(|m| m.len())
            .sum();
        eprintln!(
            "-- peer {} db: {:.1} MiB",
            peer.index,
            bytes as f64 / (1 << 20) as f64
        );
    }

    // shutting down surfaces any panic in a replica's sync task
    for (_, replica) in peers {
        replica.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

struct Peer {
    index: usize,
    db_path: PathBuf,
    keys: ([u8; 32], [u8; 32]),
    faults: Arc<Faults>,
}

impl Peer {
    /// Open (or reopen) this peer's replica on its db file. Remotes added
    /// earlier are reloaded from the db.
    async fn open(&self) -> Replica<Reducer> {
        let db = SqliteDb::open(&self.db_path).unwrap();
        let (events, _) = event_bus();
        let reducer = Reducer::new(CONTAINER_ID, "app", &[table_schema()], &db, events)
            .await
            .unwrap();
        let credentials = SoftwareCredentials::new(
            SecretBox::new(Box::new(self.keys.0)),
            SecretBox::new(Box::new(self.keys.1)),
        );
        let mut config = ReplicaConfig {
            pack_sync: fast_sync_config(),
            ..Default::default()
        };
        config.pack_remote_providers.insert(
            FAULTY.into(),
            Box::new(FaultyProvider {
                peer: self.index,
                faults: self.faults.clone(),
            }),
        );
        Replica::new(APP_ID, Box::new(db), reducer, Box::new(credentials), config)
            .await
            .unwrap()
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

fn fast_sync_config() -> PackSyncConfig {
    PackSyncConfig {
        poll_interval: Duration::from_millis(20),
        pack_retry_min: Duration::from_millis(20),
        pack_retry_max: Duration::from_secs(1),
        remote_retry_min: Duration::from_millis(20),
        remote_retry_max: Duration::from_millis(500),
        peer_publish_grace: Duration::from_millis(200),
    }
}

/// Files and bytes under a directory, recursively.
fn dir_stats(dir: &std::path::Path) -> (u64, u64) {
    let mut files = 0;
    let mut bytes = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                let (f, b) = dir_stats(&e.path());
                files += f;
                bytes += b;
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

// ── convergence ──────────────────────────────────────────────────────────────

struct PeerState {
    snapshot: SyncSnapshot,
    rows: Vec<Vec<DbValue>>,
}

impl PeerState {
    async fn read(replica: &Replica<Reducer>) -> Self {
        let snapshot = replica.sync_snapshot().await.unwrap();
        let rows = replica
            .query(r#"SELECT "k0", "v" FROM "t" ORDER BY "k0""#, &[])
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.values)
            .collect();
        Self { snapshot, rows }
    }
}

fn check_converged(groups: &[Vec<usize>], states: &[PeerState]) -> Result<(), String> {
    let mut problems = vec![];
    for group in groups {
        let first = &states[group[0]];
        for &i in group {
            let s = &states[i];
            if !s.snapshot.is_quiescent() {
                problems.push(format!(
                    "peer {i} not quiescent: blocked={:?} unpublished={}\n  {}",
                    s.snapshot.blocked,
                    s.snapshot.unpublished.len(),
                    s.snapshot.blocked_detail.join("\n  ")
                ));
            }
            if s.snapshot.streams != first.snapshot.streams {
                problems.push(format!(
                    "peer {i} stream heads differ from peer {}: {:?} vs {:?}",
                    group[0],
                    summarize(&s.snapshot),
                    summarize(&first.snapshot)
                ));
            }
            if s.rows != first.rows {
                problems.push(format!(
                    "peer {i} rows differ from peer {} ({} vs {} rows)",
                    group[0],
                    s.rows.len(),
                    first.rows.len()
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Per log: the stream head sizes, for readable failure output.
fn summarize(s: &SyncSnapshot) -> Vec<Vec<u64>> {
    let mut v: Vec<Vec<u64>> = s
        .streams
        .values()
        .map(|heads| heads.iter().map(|(size, _, _)| *size).collect())
        .collect();
    v.sort();
    v
}

/// One line per peer: the head size it holds of each author's log (`-` if none).
fn head_matrix(states: &[PeerState]) -> String {
    let authors: Vec<_> = states.iter().map(|s| s.snapshot.self_peer).collect();
    let mut out = String::new();
    for (i, s) in states.iter().enumerate() {
        let row: Vec<String> = authors
            .iter()
            .map(|a| {
                s.snapshot
                    .streams
                    .get(&(*a, CONTAINER_ID))
                    .map(|heads| {
                        heads
                            .iter()
                            .map(|(size, _, fork)| {
                                format!("{size}{}", if *fork { "f" } else { "" })
                            })
                            .collect::<Vec<_>>()
                            .join("/")
                    })
                    .unwrap_or_else(|| "-".into())
            })
            .collect();
        out += &format!("  peer {i}: {}\n", row.join(" "));
    }
    out
}

/// Peers linked (transitively) through shared remotes.
fn connected_groups(attached: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..attached.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        if p[x] != x {
            p[x] = find(p, p[x]);
        }
        p[x]
    }
    for a in 0..attached.len() {
        for b in a + 1..attached.len() {
            if attached[a].iter().any(|r| attached[b].contains(r)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..attached.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort();
    groups
}

// ── faults ───────────────────────────────────────────────────────────────────

struct Faults {
    on: AtomicBool,
    rng: Mutex<Rng>,
    p_error: f64,
    p_truncate: f64,
    /// Remote calls made per peer, to spot a stalled sync loop.
    calls: Vec<AtomicU64>,
}

impl Faults {
    fn roll(&self, p: f64) -> bool {
        self.on.load(Ordering::Relaxed) && self.rng.lock().unwrap().chance(p)
    }

    fn maybe_error(&self, op: &str, path: &str) -> Result<(), FileRemoteError> {
        if self.roll(self.p_error) {
            Err(FileRemoteError::Other(
                format!("injected {op} error: {path}").into(),
            ))
        } else {
            Ok(())
        }
    }
}

struct FaultyProvider {
    peer: usize,
    faults: Arc<Faults>,
}

#[async_trait]
impl FileRemoteProvider for FaultyProvider {
    async fn init(&self, config: &str) -> Result<Box<dyn FileRemote>, FileRemoteError> {
        Ok(Box::new(FaultyRemote {
            peer: self.peer,
            inner: StdFsRemoteProvider.init(config).await?,
            faults: self.faults.clone(),
        }))
    }
}

/// Wraps a folder remote with transient errors and truncated reads.
struct FaultyRemote {
    peer: usize,
    inner: Box<dyn FileRemote>,
    faults: Arc<Faults>,
}

#[async_trait]
impl FileRemote for FaultyRemote {
    async fn list(&self, dir: &str) -> Result<Vec<DirEntry>, FileRemoteError> {
        self.faults.calls[self.peer].fetch_add(1, Ordering::Relaxed);
        self.faults.maybe_error("list", dir)?;
        self.inner.list(dir).await
    }

    async fn read(&self, path: &str) -> Result<Option<Vec<u8>>, FileRemoteError> {
        self.faults.calls[self.peer].fetch_add(1, Ordering::Relaxed);
        self.faults.maybe_error("read", path)?;
        let mut data = self.inner.read(path).await?;
        if let Some(d) = data.as_mut()
            && self.faults.roll(self.faults.p_truncate)
        {
            d.truncate(d.len() / 2);
        }
        Ok(data)
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<(), FileRemoteError> {
        self.faults.calls[self.peer].fetch_add(1, Ordering::Relaxed);
        self.faults.maybe_error("write", path)?;
        self.inner.write(path, data).await
    }

    async fn delete(&self, path: &str) -> Result<(), FileRemoteError> {
        self.faults.calls[self.peer].fetch_add(1, Ordering::Relaxed);
        self.faults.maybe_error("delete", path)?;
        self.inner.delete(path).await
    }
}

// ── span timing ──────────────────────────────────────────────────────────────

/// Records the wall time of every closed tracing span by name (e.g. the instrumented
/// `process_pack_remotes` round and `exec`), alongside the usual WARN log output.
mod timing {
    use std::{
        collections::HashMap,
        sync::Mutex,
        time::{Duration, Instant},
    };

    use tracing::{Subscriber, span};
    use tracing_subscriber::{
        Layer, filter::LevelFilter, layer::Context, prelude::*, registry::LookupSpan,
    };

    pub type Timings = HashMap<&'static str, Vec<Duration>>;

    static TIMINGS: Mutex<Option<Timings>> = Mutex::new(None);

    struct Start(Instant);

    struct TimingLayer;

    impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for TimingLayer {
        fn on_new_span(&self, _: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
            if let Some(span) = ctx.span(id) {
                span.extensions_mut().insert(Start(Instant::now()));
            }
        }

        fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
            let Some(span) = ctx.span(&id) else { return };
            let Some(start) = span.extensions().get::<Start>().map(|s| s.0) else {
                return;
            };
            let mut timings = TIMINGS.lock().unwrap();
            timings
                .get_or_insert_with(HashMap::new)
                .entry(span.name())
                .or_default()
                .push(start.elapsed());
        }
    }

    pub fn init() {
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_test_writer()
                    .with_filter(LevelFilter::WARN),
            )
            .with(TimingLayer)
            .try_init();
    }

    pub fn reset() {
        *TIMINGS.lock().unwrap() = None;
    }

    pub fn take() -> Timings {
        TIMINGS.lock().unwrap().take().unwrap_or_default()
    }

    pub fn report(timings: &Timings) -> String {
        let mut names: Vec<_> = timings.keys().collect();
        names.sort();
        let mut out = String::new();
        for name in names {
            let mut d = timings[name].clone();
            d.sort();
            let pct = |p: f64| d[((d.len() - 1) as f64 * p) as usize];
            let total: Duration = d.iter().sum();
            out += &format!(
                "   {name}: n={} p50={:.1?} p90={:.1?} p99={:.1?} max={:.1?} total={:.1?}\n",
                d.len(),
                pct(0.5),
                pct(0.9),
                pct(0.99),
                d[d.len() - 1],
                total
            );
        }
        out
    }
}

// ── rng ──────────────────────────────────────────────────────────────────────

/// splitmix64: tiny, seedable, good enough for a simulation.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }

    fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }

    fn bytes(&mut self) -> [u8; 32] {
        let mut out = [0; 32];
        for chunk in out.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        out
    }
}
