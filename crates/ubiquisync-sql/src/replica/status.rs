//! A typed snapshot of a replica's sync state, for status screens.
//!
//! [`Replica::status`] rebuilds it from the stored tables (branches, published sizes and
//! pack read state), so it is right after a restart. Only whether each sync folder is
//! reachable, when it was last read and written, and the state of the background tasks are
//! kept in memory.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures::FutureExt;
use sea_query::Query;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use ubiquisync_core::{
    crypto::RootKey256Fingerprint,
    hlc::Timestamp,
    ids::{ContainerId, PeerId},
    pack::PackFileId,
};

use crate::{
    db::{DbError, sea_query::select_cols},
    replica::{
        Replica, ReplicaInner,
        config::backoff_delay,
        events::{SyncEvent, SyncEventKind, event_channel},
        fs_sync_schema::{BlockedReasons, pack_read_state, published, remotes},
        hlc::MAX_SKEW_MS,
        schema::{CommitErr, HeadErr, UnknownSoftwareVersion, branches, peers},
    },
};

/// The local id of a sync folder, as returned by `add_remote`.
pub type RemoteId = i64;

/// Everything a status screen needs, in one call.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncStatus {
    /// The one-line answer.
    pub level: Level,
    pub this_device: PeerId,
    /// Whether sync with every folder is paused, see [`Replica::set_sync_paused`].
    pub paused: bool,
    /// Entries this device holds that are not yet in a sync folder: the most over any one
    /// folder. Includes other devices' entries this device would relay.
    pub to_send: u64,
    /// Entries received but not applied yet, over every log.
    pub to_apply: u64,
    /// One per (device, container), sorted with this device first.
    pub logs: Vec<LogStatus>,
    pub remotes: Vec<RemoteStatus>,
    pub tasks: Vec<TaskStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Nothing to sync with yet.
    NoRemote,
    /// Nothing to send, nothing to apply, no stall.
    UpToDate,
    /// Entries to send or apply, every folder reachable.
    Working,
    /// Sync is paused on this device (see [`Replica::set_sync_paused`]).
    Paused,
    /// Every sync folder is unreachable.
    Offline,
    /// A log is stalled, a file is blocked, or a background task stopped.
    Attention,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogStatus {
    pub device: PeerId,
    pub container: ContainerId,
    pub is_this_device: bool,
    /// Entries stored (`branches.head_size` of the log's main branch).
    pub received: u64,
    /// Entries applied to the tables (`branches.commit_size`).
    pub applied: u64,
    /// Entries of the main branch found in every attached sync folder (the least over
    /// folders, 0 with none). For this device's own log this is "published".
    pub in_every_remote: u64,
    /// The author's clock on the newest entry. Not filled in yet.
    pub last_entry_at: Option<Timestamp>,
    pub stall: Option<Stall>,
    /// Empty unless this log forked.
    pub forks: Vec<ForkStatus>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ForkStatus {
    /// The size of the log where the fork branched off.
    pub forked_at: u64,
    pub received: u64,
    pub applied: u64,
    pub stall: Option<Stall>,
}

/// Why a log is not applying entries.
#[derive(Debug, Clone, PartialEq)]
pub enum Stall {
    /// An internal error; retried with backoff.
    Retrying { attempts: u64, next_try: SystemTime },
    /// The entry's clock is too far ahead of this device's. It applies once this
    /// device's clock reaches `until`.
    ClockAhead {
        entry_at: Timestamp,
        until: SystemTime,
    },
    /// The entry needs a newer version of the app.
    NeedsNewerApp(UnknownSoftwareVersion),
    /// Waiting for entries of another log.
    WaitingForOtherLog,
    /// The key to decrypt the entries is missing.
    MissingKey(RootKey256Fingerprint),
    /// The log cannot be applied any further.
    Frozen,
    /// Entries are only accepted up to this size.
    BlockedAt(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteStatus {
    pub id: RemoteId,
    pub provider: String,
    pub config: String,
    pub reach: Reach,
    /// The last round that read the folder without error. Kept in memory only.
    pub last_read_at: Option<SystemTime>,
    /// The last file this device wrote to the folder. Kept in memory only.
    pub last_publish_at: Option<SystemTime>,
    /// Entries this device holds that are not in this folder yet.
    pub to_send: u64,
    /// Files read in full, over every publisher (including this device's own).
    pub files_read: u64,
    pub blocked: Vec<BlockedPack>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Reach {
    #[default]
    Ok,
    Unreachable {
        since: SystemTime,
        /// Failed rounds in a row.
        attempts: u64,
        /// `None` when the folder could not be opened at startup; it is not retried until
        /// the next start.
        next_try: Option<SystemTime>,
        /// For display only.
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockedPack {
    pub publisher: PeerId,
    pub file: PackFileId,
    pub attempts: u64,
    pub reasons: BlockedReasons,
    /// Earlier files this one waits for.
    pub waiting_on_files: u64,
    /// Devices named in the file whose init file is missing.
    pub unknown_devices: Vec<PeerId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub task: Task,
    pub state: TaskState,
    /// How many times the task was restarted after stopping.
    pub restarts: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Task {
    /// Reads and writes the sync folders.
    PackSync,
    /// Applies entries whose first try failed or fell behind.
    CommitRetry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    /// The task stopped with this error (for display) and is waiting to be restarted.
    Stopped {
        error: String,
    },
}

// ── in-memory state ─────────────────────────────────────────────────────────

/// The part of the status that is not stored, shared between the background tasks and
/// `status()`.
pub(crate) struct SyncRuntime {
    pub events: broadcast::Sender<SyncEvent>,
    remotes: Mutex<HashMap<RemoteId, RemoteRuntime>>,
    tasks: Mutex<BTreeMap<Task, TaskStatus>>,
    paused: AtomicBool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteRuntime {
    pub reach: Reach,
    pub last_read_at: Option<SystemTime>,
    pub last_publish_at: Option<SystemTime>,
}

impl SyncRuntime {
    pub(crate) fn new() -> Self {
        Self {
            events: event_channel(),
            remotes: Default::default(),
            tasks: Default::default(),
            paused: AtomicBool::new(false),
        }
    }

    pub(crate) fn remote(&self, id: RemoteId) -> RemoteRuntime {
        lock(&self.remotes).get(&id).cloned().unwrap_or_default()
    }

    pub(crate) fn update_remote(&self, id: RemoteId, f: impl FnOnce(&mut RemoteRuntime)) {
        f(lock(&self.remotes).entry(id).or_default())
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    fn set_task(&self, task: Task, state: TaskState, restarts: u64) {
        lock(&self.tasks).insert(
            task,
            TaskStatus {
                task,
                state,
                restarts,
            },
        );
    }
}

fn lock<G>(m: &Mutex<G>) -> MutexGuard<'_, G> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn to_system_time(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

// ── task supervision ────────────────────────────────────────────────────────

/// The shortest and longest delay before a stopped background task is restarted.
const TASK_RESTART_MIN: Duration = Duration::from_secs(1);
const TASK_RESTART_MAX: Duration = Duration::from_secs(60);

impl<R: Send + Sync + 'static> Replica<R> {
    /// Runs a background loop in the replica's task set. If the loop panics, the panic is
    /// recorded (so `status()` shows it and `watch()` sends `TaskStopped`) and the loop is
    /// started again after a growing delay. It ends for good when the replica shuts down.
    pub(crate) fn spawn_supervised<F, Fut>(&mut self, task: Task, make: F)
    where
        F: Fn(std::sync::Arc<ReplicaInner<R>>, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let inner = self.inner.clone();
        let cancel = self.cancel.clone();
        inner.sync.set_task(task, TaskState::Running, 0);
        self.tasks.spawn(async move {
            let mut restarts = 0u64;
            loop {
                let run = std::panic::AssertUnwindSafe(make(inner.clone(), cancel.clone()));
                match run.catch_unwind().await {
                    Ok(()) => return, // the loop only returns when cancelled
                    Err(panic) => {
                        let error = panic_message(&panic);
                        tracing::error!(?task, error, "background task stopped, restarting");
                        inner.sync.set_task(
                            task,
                            TaskState::Stopped {
                                error: error.clone(),
                            },
                            restarts,
                        );
                        inner.emit(SyncEventKind::TaskStopped { task, error });
                    }
                }
                let delay = backoff_delay(TASK_RESTART_MIN, TASK_RESTART_MAX, restarts);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                restarts += 1;
                inner.sync.set_task(task, TaskState::Running, restarts);
            }
        });
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked".to_string()
    }
}

// ── pause ───────────────────────────────────────────────────────────────────

impl<R> Replica<R> {
    /// Pauses or resumes reading and writing every sync folder. Local writes and applying
    /// entries already received carry on. Not stored: a restarted replica syncs.
    pub fn set_sync_paused(&self, paused: bool) {
        self.inner.sync.paused.store(paused, Ordering::Relaxed);
    }

    pub fn sync_paused(&self) -> bool {
        self.inner.sync.paused()
    }
}

// ── snapshot ────────────────────────────────────────────────────────────────

/// One `branches` row with its author.
struct BranchRow {
    id: i64,
    device: PeerId,
    container: ContainerId,
    head_size: u64,
    head_err: Option<HeadErr>,
    commit_size: u64,
    commit_err: Option<CommitErr>,
    parent_id: Option<i64>,
    fork_size: Option<u64>,
}

impl<R> Replica<R> {
    /// A snapshot of the sync state. A handful of small queries, cheap enough to call on
    /// every event from [`watch`](Self::watch).
    pub async fn status(&self) -> Result<SyncStatus, DbError> {
        self.inner.status().await
    }
}

impl<R> ReplicaInner<R> {
    pub(crate) async fn status(&self) -> Result<SyncStatus, DbError> {
        let db = self.db.as_ref();
        let this_device = self.self_init.peer_id;

        let peer_rows =
            select_cols::<(peers::Id, peers::PeerId)>(db, Query::select().from(peers::Table))
                .await?;
        let mut peer_ids = HashMap::new();
        for row in peer_rows.iter() {
            let (id, peer) = row?;
            peer_ids.insert(id, PeerId(peer));
        }

        let rows = select_cols::<(
            branches::Id,
            branches::PeerId,
            branches::ContainerId,
            branches::HeadSize,
            branches::HeadErr,
            branches::CommitSize,
            branches::CommitErr,
            branches::ParentId,
            branches::ForkSize,
        )>(db, Query::select().from(branches::Table))
        .await?;
        let mut branch_rows = vec![];
        for row in rows.iter() {
            let (
                id,
                peer_db_id,
                container,
                head_size,
                head_err,
                commit_size,
                commit_err,
                parent_id,
                fork_size,
            ) = row?;
            let Some(device) = peer_ids.get(&peer_db_id).copied() else {
                continue;
            };
            branch_rows.push(BranchRow {
                id,
                device,
                container: ContainerId(container),
                head_size,
                head_err,
                commit_size,
                commit_err,
                parent_id,
                fork_size,
            });
        }

        let remote_rows = select_cols::<(remotes::Id, remotes::Provider, remotes::Config)>(
            db,
            Query::select()
                .from(remotes::Table)
                .order_by(remotes::Id, sea_query::Order::Asc),
        )
        .await?;
        let remote_list: Vec<(RemoteId, String, String)> = remote_rows
            .iter()
            .map(|r| r.map(|(id, p, c)| (id, p.to_string(), c.to_string())))
            .collect::<Result<_, _>>()?;

        // published size per (remote, branch)
        let pub_rows = select_cols::<(
            published::RemoteId,
            published::BranchId,
            published::PublishedSize,
        )>(db, Query::select().from(published::Table))
        .await?;
        let mut published_sizes: HashMap<(RemoteId, i64), u64> = HashMap::new();
        for row in pub_rows.iter() {
            let (remote, branch, size) = row?;
            published_sizes.insert((remote, branch), size);
        }

        let read_rows = select_cols::<(
            pack_read_state::RemoteId,
            pack_read_state::PeerId,
            pack_read_state::State,
        )>(db, Query::select().from(pack_read_state::Table))
        .await?;
        let mut files_read: HashMap<RemoteId, u64> = HashMap::new();
        let mut blocked: HashMap<RemoteId, Vec<BlockedPack>> = HashMap::new();
        for row in read_rows.iter() {
            let (remote, peer_db_id, state) = row?;
            *files_read.entry(remote).or_default() += state.consumed.len() as u64;
            let Some(publisher) = peer_ids.get(&peer_db_id).copied() else {
                continue;
            };
            for b in state.blocked.values() {
                blocked.entry(remote).or_default().push(BlockedPack {
                    publisher,
                    file: b.file.clone(),
                    attempts: b.attempts,
                    reasons: b.reasons,
                    waiting_on_files: b.pending_parents.len() as u64,
                    unknown_devices: b.missing_peers.clone(),
                });
            }
        }

        let loaded: Vec<RemoteId> = self.pack_remotes.lock().await.keys().copied().collect();

        // remotes
        let mut remote_statuses = vec![];
        for (id, provider, config) in remote_list {
            let rt = self.sync.remote(id);
            let to_send = branch_rows
                .iter()
                .map(|b| {
                    b.head_size
                        .saturating_sub(published_sizes.get(&(id, b.id)).copied().unwrap_or(0))
                })
                .sum();
            let mut blocked = blocked.remove(&id).unwrap_or_default();
            blocked.sort_by_key(|b| (b.publisher, b.file.seqs.start, b.file.id));
            let reach = if loaded.contains(&id) {
                rt.reach
            } else {
                match rt.reach {
                    r @ Reach::Unreachable { .. } => r,
                    Reach::Ok => Reach::Unreachable {
                        since: SystemTime::now(),
                        attempts: 0,
                        next_try: None,
                        error: "not opened".into(),
                    },
                }
            };
            remote_statuses.push(RemoteStatus {
                id,
                provider,
                config,
                reach,
                last_read_at: rt.last_read_at,
                last_publish_at: rt.last_publish_at,
                to_send,
                files_read: files_read.get(&id).copied().unwrap_or(0),
                blocked,
            });
        }

        // logs: the root branch of each (device, container), with its forks
        let retry_cfg = &self.config.commit_retry;
        let stall_of = |b: &BranchRow| -> Option<Stall> {
            if let Some(e) = &b.commit_err {
                return Some(stall_from_commit_err(e, retry_cfg));
            }
            b.head_err.map(|e| match e {
                HeadErr::Blocked(n) => Stall::BlockedAt(n),
                HeadErr::NeedUseKey(k) => Stall::MissingKey(k),
            })
        };
        let mut logs: BTreeMap<(bool, PeerId, [u8; 16]), LogStatus> = BTreeMap::new();
        for b in branch_rows.iter().filter(|b| b.parent_id.is_none()) {
            let in_every_remote = remote_statuses
                .iter()
                .map(|r| {
                    published_sizes
                        .get(&(r.id, b.id))
                        .copied()
                        .unwrap_or(0)
                        .min(b.head_size)
                })
                .min()
                .unwrap_or(0);
            logs.insert(
                (b.device != this_device, b.device, b.container.0),
                LogStatus {
                    device: b.device,
                    container: b.container,
                    is_this_device: b.device == this_device,
                    received: b.head_size,
                    applied: b.commit_size,
                    in_every_remote,
                    last_entry_at: None,
                    stall: stall_of(b),
                    forks: vec![],
                },
            );
        }
        for b in branch_rows.iter().filter(|b| b.parent_id.is_some()) {
            let key = (b.device != this_device, b.device, b.container.0);
            if let Some(log) = logs.get_mut(&key) {
                log.forks.push(ForkStatus {
                    forked_at: b.fork_size.unwrap_or(0),
                    received: b.head_size,
                    applied: b.commit_size,
                    stall: stall_of(b),
                });
            }
        }
        let mut logs: Vec<LogStatus> = logs.into_values().collect();
        for log in &mut logs {
            log.forks.sort_by_key(|f| f.forked_at);
        }

        let to_apply = branch_rows
            .iter()
            .map(|b| b.head_size.saturating_sub(b.commit_size))
            .sum();
        let to_send = remote_statuses.iter().map(|r| r.to_send).max().unwrap_or(0);
        let tasks: Vec<TaskStatus> = lock(&self.sync.tasks).values().cloned().collect();
        let paused = self.sync.paused();

        let any_stall = logs
            .iter()
            .any(|l| l.stall.is_some() || l.forks.iter().any(|f| f.stall.is_some()));
        let any_blocked = remote_statuses.iter().any(|r| !r.blocked.is_empty());
        let any_stopped = tasks
            .iter()
            .any(|t| matches!(t.state, TaskState::Stopped { .. }));
        let level = if any_stall || any_blocked || any_stopped {
            Level::Attention
        } else if remote_statuses.is_empty() {
            Level::NoRemote
        } else if paused {
            Level::Paused
        } else if remote_statuses
            .iter()
            .all(|r| matches!(r.reach, Reach::Unreachable { .. }))
        {
            Level::Offline
        } else if to_send + to_apply > 0 {
            Level::Working
        } else {
            Level::UpToDate
        };

        Ok(SyncStatus {
            level,
            this_device,
            paused,
            to_send,
            to_apply,
            logs,
            remotes: remote_statuses,
            tasks,
        })
    }
}

pub(crate) fn stall_from_commit_err(
    e: &CommitErr,
    cfg: &crate::replica::CommitRetryConfig,
) -> Stall {
    match e {
        CommitErr::Internal {
            retry_time_span,
            retry_count,
        } => {
            let delay = backoff_delay(cfg.retry_min, cfg.retry_max, *retry_count);
            Stall::Retrying {
                attempts: retry_count + 1,
                next_try: to_system_time(retry_time_span.end) + delay,
            }
        }
        CommitErr::HLCForwardSkew(ts) => Stall::ClockAhead {
            entry_at: *ts,
            until: to_system_time(ts.wall().as_millis().saturating_sub(MAX_SKEW_MS)),
        },
        CommitErr::NeedKey(k) => Stall::MissingKey(*k),
        CommitErr::AwaitingDeps => Stall::WaitingForOtherLog,
        CommitErr::IncompatibleSoftware(v) => Stall::NeedsNewerApp(*v),
        CommitErr::Frozen => Stall::Frozen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replica::CommitRetryConfig;
    use ubiquisync_core::hlc::WallTime;

    #[test]
    fn clock_ahead_applies_once_within_the_skew_window() {
        let ts = Timestamp::from_parts(WallTime::from_millis(1_000_000), 0).unwrap();
        let stall = stall_from_commit_err(&CommitErr::HLCForwardSkew(ts), &Default::default());
        assert_eq!(
            stall,
            Stall::ClockAhead {
                entry_at: ts,
                until: to_system_time(1_000_000 - MAX_SKEW_MS)
            }
        );
    }

    #[test]
    fn internal_error_retries_after_its_backoff() {
        let cfg = CommitRetryConfig {
            retry_min: Duration::from_secs(10),
            retry_max: Duration::from_secs(60),
            ..Default::default()
        };
        let err = CommitErr::Internal {
            retry_time_span: 1_000..5_000,
            retry_count: 2,
        };
        assert_eq!(
            stall_from_commit_err(&err, &cfg),
            Stall::Retrying {
                attempts: 3,
                next_try: to_system_time(5_000 + 40_000)
            }
        );
    }
}
