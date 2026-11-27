//! The two demo devices: real `StoreImpl`s in this process, each on its own SQLite file,
//! both attached to one local sync folder.
//!
//! All engine work runs on a tokio runtime owned here, so the UI can await it from any
//! executor.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use secrecy::SecretBox;
use ubiquisync_core::{
    crypto::credentials::software::SoftwareCredentials,
    ids::{LogId, PeerId},
    pack::StdFsRemoteProvider,
};
use ubiquisync_sql::{
    Exec,
    replica::{
        CommitRetryConfig, EntryView, PackSyncConfig, RemoteFile, RemoteId, ReplicaConfig,
        SyncStatus,
    },
};
use ubiquisync_sqlite::SqliteDb;
use ubiquisync_tables::{codec::OpView, codec::decode_any, op::Op, store::StoreImpl};

use crate::schema::{APP_ID, CONTAINER_ID, todos};

const PROVIDER: &str = "folder";

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `fut` on the engine runtime and waits for it from whatever executor calls this.
pub async fn on_engine<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    runtime().spawn(fut).await.expect("engine task panicked")
}

pub async fn sleep(d: Duration) {
    on_engine(tokio::time::sleep(d)).await
}

/// Where the demo keeps its files: `UBIQUISYNC_DEMO_DIR`, or `ubiquisync-explorer-demo`
/// in the system temp directory.
pub fn demo_dir() -> PathBuf {
    std::env::var_os("UBIQUISYNC_DEMO_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("ubiquisync-explorer-demo"))
}

pub struct Device {
    /// "A" or "B".
    pub name: &'static str,
    pub id: PeerId,
    pub store: StoreImpl,
    pub remote: RemoteId,
}

pub struct Engine {
    pub dir: PathBuf,
    pub folder: PathBuf,
    pub devices: Vec<Device>,
    /// Bumped on every reset, so the UI starts over.
    pub generation: u64,
}

impl PartialEq for Engine {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl Engine {
    pub async fn open(dir: PathBuf, generation: u64) -> Result<Self, String> {
        let folder = dir.join("sync-folder");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let mut devices = vec![];
        for name in ["A", "B"] {
            devices.push(open_device(&dir, &folder, name).await?);
        }
        Ok(Self {
            dir,
            folder,
            devices,
            generation,
        })
    }

    /// Shuts both devices down, deletes the demo directory, and opens fresh devices.
    pub async fn reset(self) -> Result<Self, String> {
        let (dir, generation) = (self.dir.clone(), self.generation);
        for d in self.devices {
            d.store.shutdown().await;
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        Engine::open(dir, generation + 1).await
    }

    pub fn name_of(&self, peer: &PeerId) -> String {
        self.devices
            .iter()
            .find(|d| d.id == *peer)
            .map(|d| format!("Device {}", d.name))
            .unwrap_or_else(|| format!("Device {}", short_id(peer)))
    }

    /// 0 for A, 1 for B, `None` for a device this demo did not create.
    pub fn index_of(&self, peer: &PeerId) -> Option<usize> {
        self.devices.iter().position(|d| d.id == *peer)
    }
}

async fn open_device(dir: &Path, folder: &Path, name: &'static str) -> Result<Device, String> {
    let keys = load_or_create_keys(&dir.join(format!("device-{name}.keys")))?;
    let db = SqliteDb::open(dir.join(format!("device-{name}.db"))).map_err(|e| e.to_string())?;
    let credentials = SoftwareCredentials::new(
        SecretBox::new(Box::new(keys.0)),
        SecretBox::new(Box::new(keys.1)),
    );
    let tables = crate::schema::tables().map_err(|e| e.to_string())?;
    // quick timings so the demo reacts within a second
    let config = ReplicaConfig {
        pack_sync: PackSyncConfig {
            poll_interval: Duration::from_millis(500),
            pack_retry_min: Duration::from_secs(1),
            pack_retry_max: Duration::from_secs(30),
            remote_retry_min: Duration::from_secs(1),
            remote_retry_max: Duration::from_secs(30),
            peer_publish_grace: Duration::from_secs(30),
        },
        commit_retry: CommitRetryConfig {
            poll_interval: Duration::from_secs(1),
            retry_min: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
        },
    };
    let store = StoreImpl::new(
        APP_ID,
        CONTAINER_ID,
        Box::new(credentials),
        "app",
        &tables,
        Box::new(db),
        [(PROVIDER.to_string(), Box::new(StdFsRemoteProvider) as _)].into(),
        config,
    )
    .await
    .map_err(|e| e.to_string())?;
    let status = store.status().await.map_err(|e| e.to_string())?;
    // the db remembers the folder, attach it only the first time
    let remote = match status.remotes.first() {
        Some(r) => r.id,
        None => store
            .add_remote(PROVIDER, folder.to_str().ok_or("folder path is not UTF-8")?)
            .await
            .map_err(|e| e.to_string())?,
    };
    Ok(Device {
        name,
        id: status.this_device,
        store,
        remote,
    })
}

/// The device's signing and encryption keys, kept in a plain file. Fine for a demo,
/// never for real data.
fn load_or_create_keys(path: &Path) -> Result<([u8; 32], [u8; 32]), String> {
    if let Ok(b) = std::fs::read(path)
        && b.len() == 64
    {
        let mut k = ([0; 32], [0; 32]);
        k.0.copy_from_slice(&b[..32]);
        k.1.copy_from_slice(&b[32..]);
        return Ok(k);
    }
    let mut b = [0u8; 64];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    std::fs::write(path, b).map_err(|e| e.to_string())?;
    let mut k = ([0; 32], [0; 32]);
    k.0.copy_from_slice(&b[..32]);
    k.1.copy_from_slice(&b[32..]);
    Ok(k)
}

pub fn new_todo_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).expect("random bytes");
    id
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// `a3f1 7c02 … 9c4e`
pub fn short_id(peer: &PeerId) -> String {
    let h = hex::encode(peer.0);
    format!("{} {} … {}", &h[0..4], &h[4..8], &h[h.len() - 4..])
}

// ── what the UI shows for one device ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Todo {
    pub id: [u8; 16],
    pub title: String,
    pub done: bool,
    pub created_at: i64,
}

/// One log entry as this device holds it, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub device: PeerId,
    pub entry: EntryView,
    pub op: Option<OpView>,
}

/// Everything the views show for one device, read in one go.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceView {
    pub status: Option<SyncStatus>,
    pub todos: Vec<Todo>,
    /// The files in the sync folder, with whether this device has read each one.
    pub files: Vec<RemoteFile>,
    /// Every entry of every log this device holds, per log in index order.
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

impl DeviceView {
    pub async fn load(engine: Arc<Engine>, index: usize) -> DeviceView {
        on_engine(async move {
            let d = &engine.devices[index];
            match load_view(d).await {
                Ok(v) => v,
                Err(e) => DeviceView {
                    error: Some(e),
                    ..Default::default()
                },
            }
        })
        .await
    }
}

async fn load_view(d: &Device) -> Result<DeviceView, String> {
    let status = d.store.status().await.map_err(|e| e.to_string())?;
    let mut todos: Vec<Todo> = todos::get_all(&d.store)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| Todo {
            id: r.id,
            title: r.title.unwrap_or_default(),
            done: r.done.unwrap_or(0) != 0,
            created_at: r.created_at.unwrap_or(0),
        })
        .collect();
    todos.sort_by_key(|t| (t.created_at, t.id));
    let files = d.store.remote_files(d.remote).await.unwrap_or_default();
    let mut entries = vec![];
    for log in &status.logs {
        let id = LogId {
            peer_id: log.device,
            container_id: log.container,
        };
        for e in d
            .store
            .entries(id, 0..u64::MAX)
            .await
            .map_err(|e| e.to_string())?
        {
            let op = e.op.as_deref().and_then(|b| decode_any(b).ok());
            entries.push(Entry {
                device: log.device,
                entry: e,
                op,
            });
        }
    }
    Ok(DeviceView {
        status: Some(status),
        todos,
        files,
        entries,
        error: None,
    })
}

// ── writes ──────────────────────────────────────────────────────────────────

pub async fn exec(engine: Arc<Engine>, index: usize, op: Op) -> Result<(), String> {
    on_engine(async move {
        engine.devices[index]
            .store
            .exec(None, op)
            .await
            .map_err(|e| e.to_string())
    })
    .await
}

/// Adds a todo on device A and waits until device B has applied it and both are up to
/// date. For checking the demo works without opening a window.
pub async fn smoke_test(engine: Arc<Engine>) -> Result<String, String> {
    use ubiquisync_sql::replica::Level;
    let id = new_todo_id();
    let title = format!("smoke test {}", now_ms());
    exec(
        engine.clone(),
        0,
        todos::upsert(&id)
            .title(&title)
            .done(0)
            .created_at(now_ms())
            .build(),
    )
    .await?;
    let started = std::time::Instant::now();
    loop {
        let b = DeviceView::load(engine.clone(), 1).await;
        let a = DeviceView::load(engine.clone(), 0).await;
        if let Some(e) = a.error.or(b.error.clone()) {
            return Err(e);
        }
        let seen = b.todos.iter().any(|t| t.title == title);
        let levels = (
            a.status.as_ref().map(|s| s.level),
            b.status.as_ref().map(|s| s.level),
        );
        if seen && levels == (Some(Level::UpToDate), Some(Level::UpToDate)) {
            return Ok(format!(
                "B shows {:?} after {:?}; {} entries in B's log view, {} files in the folder",
                title,
                started.elapsed(),
                b.entries.len(),
                b.files.len()
            ));
        }
        if started.elapsed() > Duration::from_secs(20) {
            return Err(format!(
                "not synced after 20 s: seen={seen} levels={levels:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
