//! Small pieces shared by the views: device markers, icons, and text for engine values.

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use dioxus::prelude::*;
use ubiquisync_core::{hlc::Timestamp, ids::PeerId};
use ubiquisync_sql::replica::{BlockedReasons, Stall, SyncStatus, Task, UnknownSoftwareVersion};

use super::Shared;
use crate::engine::Engine;

/// "1 entry", "3 entries".
pub fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

pub fn size(bytes: u64) -> String {
    if bytes < 1000 {
        format!("{bytes} B")
    } else {
        format!("{:.1} kB", bytes as f64 / 1000.0)
    }
}

pub fn clock(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%H:%M:%S")
        .to_string()
}

pub fn clock_ms(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%H:%M:%S%.3f")
        .to_string()
}

pub fn ts_time(ts: Timestamp) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ts.wall().as_millis())
}

/// "just now", "12 s ago", "2 min ago", "in 4 min".
pub fn relative(t: SystemTime) -> String {
    let now = SystemTime::now();
    let (secs, future) = match now.duration_since(t) {
        Ok(d) => (d.as_secs(), false),
        Err(e) => (e.duration().as_secs(), true),
    };
    let amount = if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min", secs / 60)
    } else {
        format!("{} h", secs / 3600)
    };
    match (secs, future) {
        (0..=2, _) => "just now".into(),
        (_, true) => format!("in {amount}"),
        (_, false) => format!("{amount} ago"),
    }
}

/// The short label and a longer detail for a stall.
pub fn stall_text(s: &Stall) -> (String, String) {
    match s {
        Stall::Retrying { attempts, next_try } => (
            "Retrying after an error".into(),
            format!("attempt {attempts}, next try {}", clock(*next_try)),
        ),
        Stall::ClockAhead { until, .. } => (
            "Waiting: clock ahead".into(),
            format!("its entries apply at {}", clock(*until)),
        ),
        Stall::NeedsNewerApp(v) => (
            "Needs a newer app".into(),
            match v {
                UnknownSoftwareVersion::EntryType(t) => format!("unknown entry type {t}"),
                UnknownSoftwareVersion::OpType(t) => format!("unknown op type {t}"),
                UnknownSoftwareVersion::CipherSuite(t) => format!("unknown cipher suite {t}"),
            },
        ),
        Stall::WaitingForOtherLog => (
            "Waiting for another log".into(),
            "an entry depends on entries not received yet".into(),
        ),
        Stall::MissingKey(k) => (
            "Missing a key".into(),
            format!("key {}", hex::encode(&k.0[..4])),
        ),
        Stall::Frozen => ("Stopped".into(), "its entries cannot be applied".into()),
        Stall::BlockedAt(n) => ("Blocked".into(), format!("accepts entries up to {n} only")),
    }
}

pub fn blocked_reasons(r: BlockedReasons) -> String {
    let mut out = vec![];
    if r.needs_upgrade() {
        out.push("needs a newer app");
    }
    if r.bad_metadata() {
        out.push("contents do not match the header");
    }
    if r.decode_error() {
        out.push("does not decode");
    }
    if r.bad_body_hash() {
        out.push("body incomplete or changed");
    }
    if r.bad_signature() {
        out.push("bad signature");
    }
    if r.corrupt_header() {
        out.push("header incomplete or corrupt");
    }
    if out.is_empty() {
        "waiting for earlier files".into()
    } else {
        out.join(", ")
    }
}

pub fn task_name(t: Task) -> &'static str {
    match t {
        Task::PackSync => "Folder sync",
        Task::CommitRetry => "Apply retry",
    }
}

/// How many files in the folder, from other devices, this device has not read in full.
pub fn files_to_read(engine: &Engine, index: usize, view: &crate::engine::DeviceView) -> usize {
    let me = engine.devices[index].id;
    view.files
        .iter()
        .filter(|f| f.publisher != me && !f.read)
        .count()
}

/// The marker for a device: A a blue square, B an orange circle, others a teal diamond.
#[component]
pub fn Mark(engine: Arc<Engine>, device: PeerId) -> Element {
    let class = match engine.index_of(&device) {
        Some(0) => "mark a",
        Some(1) => "mark b",
        _ => "mark other",
    };
    rsx! { span { class: "{class}", "aria-hidden": "true" } }
}

/// "A", "B", or a short id.
pub fn letter(engine: &Engine, device: &PeerId) -> String {
    match engine.index_of(device) {
        Some(i) => engine.devices[i].name.to_string(),
        None => hex::encode(&device.0[..2]),
    }
}

pub fn device_class(engine: &Engine, device: &PeerId) -> &'static str {
    match engine.index_of(device) {
        Some(0) => "a",
        Some(1) => "b",
        _ => "other",
    }
}

#[component]
pub fn CheckIcon() -> Element {
    rsx! {
        svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
            stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "m5 12 4.5 4.5L19 7" }
        }
    }
}

#[component]
pub fn PauseIcon() -> Element {
    rsx! {
        svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
            stroke_width: "2.2", stroke_linecap: "round", "aria-hidden": "true",
            path { d: "M9 6v12M15 6v12" }
        }
    }
}

#[component]
pub fn WarnIcon() -> Element {
    rsx! {
        svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
            stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M12 4 3 20h18z" }
            path { d: "M12 10v4M12 17h.01" }
        }
    }
}

#[component]
pub fn SyncIcon() -> Element {
    rsx! {
        svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
            stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M20 12a8 8 0 0 1-14 5.3M4 12a8 8 0 0 1 14-5.3" }
            path { d: "M18 3v4h-4M6 21v-4h4" }
        }
    }
}

#[component]
pub fn ClockIcon() -> Element {
    rsx! {
        svg { width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor",
            stroke_width: "2.2", stroke_linecap: "round", "aria-hidden": "true",
            circle { cx: "12", cy: "12", r: "8" }
            path { d: "M12 8v4l3 2" }
        }
    }
}

/// The CSS class, icon, headline and detail for a status's level.
pub struct LevelLook {
    pub class: &'static str,
    pub icon: Element,
    pub headline: String,
    pub detail: String,
}

pub fn level_look(engine: &Engine, s: &SyncStatus, files_to_read: usize) -> LevelLook {
    use ubiquisync_sql::replica::{Level, Reach, TaskState};
    let pending = {
        let mut parts = vec![];
        if s.to_send > 0 {
            parts.push(format!("{} to send", s.to_send));
        }
        if s.to_apply > 0 {
            parts.push(format!("{} to apply", s.to_apply));
        }
        if files_to_read > 0 {
            parts.push(count(files_to_read as u64, "file to read", "files to read"));
        }
        parts
    };
    let with_pending = |head: &str| {
        let mut v = vec![head.to_string()];
        v.extend(pending.iter().cloned());
        v.join(" · ")
    };
    match s.level {
        Level::UpToDate => LevelLook {
            class: "lvl-ok",
            icon: rsx! { CheckIcon {} },
            headline: "Up to date".into(),
            detail:
                "Everything written here is in the sync folder, and everything received is applied."
                    .into(),
        },
        Level::Working => LevelLook {
            class: "lvl-work",
            icon: rsx! { SyncIcon {} },
            headline: with_pending("Syncing"),
            detail: "Entries are on their way to or from the sync folder.".into(),
        },
        Level::Paused => LevelLook {
            class: "lvl-warn",
            icon: rsx! { PauseIcon {} },
            headline: with_pending("Paused (offline)"),
            detail: "Sync is paused on this device. Edits stay here until it resumes.".into(),
        },
        Level::Offline => LevelLook {
            class: "lvl-warn",
            icon: rsx! { WarnIcon {} },
            headline: with_pending("Offline"),
            detail: s
                .remotes
                .iter()
                .find_map(|r| match &r.reach {
                    Reach::Unreachable { error, .. } => {
                        Some(format!("The sync folder cannot be reached: {error}"))
                    }
                    Reach::Ok => None,
                })
                .unwrap_or_default(),
        },
        Level::NoRemote => LevelLook {
            class: "lvl-none",
            icon: rsx! { WarnIcon {} },
            headline: "No sync folder".into(),
            detail: "This device has nothing to sync with.".into(),
        },
        Level::Attention => {
            let detail = if let Some((log, stall)) = s.logs.iter().find_map(|l| {
                l.stall
                    .as_ref()
                    .or(l.forks.iter().find_map(|f| f.stall.as_ref()))
                    .map(|st| (l, st))
            }) {
                let (short, long) = stall_text(stall);
                let who = if log.is_this_device {
                    "This device".to_string()
                } else {
                    engine.name_of(&log.device)
                };
                format!("{who} is waiting: {} ({long}).", short.to_lowercase())
            } else if let Some(b) = s.remotes.iter().find_map(|r| r.blocked.first()) {
                format!(
                    "A file from {} cannot be read yet: {}.",
                    engine.name_of(&b.publisher),
                    blocked_reasons(b.reasons)
                )
            } else if let Some(t) = s
                .tasks
                .iter()
                .find(|t| matches!(t.state, TaskState::Stopped { .. }))
            {
                let TaskState::Stopped { error } = &t.state else {
                    unreachable!()
                };
                format!(
                    "{} stopped ({error}); it restarts shortly.",
                    task_name(t.task)
                )
            } else {
                String::new()
            };
            LevelLook {
                class: "lvl-warn",
                icon: rsx! { WarnIcon {} },
                headline: with_pending("Needs attention"),
                detail,
            }
        }
    }
}

/// The "Viewing as" device picker for the Status and Logs views.
#[component]
pub fn ViewAs(engine: Arc<Engine>, shared: Shared) -> Element {
    let mut view_as = shared.view_as;
    rsx! {
        label { style: "display: flex; align-items: center; gap: 8px;",
            "Viewing as"
            select {
                "aria-label": "Viewing as device",
                onchange: move |e| view_as.set(e.value().parse().unwrap_or(0)),
                for (i, d) in engine.devices.iter().enumerate() {
                    option { value: "{i}", selected: view_as() == i, "Device {d.name}" }
                }
            }
        }
    }
}
