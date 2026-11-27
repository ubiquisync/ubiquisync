//! "Status": the `status()` snapshot of one device, and its activity feed.

use std::sync::Arc;

use dioxus::prelude::*;
use ubiquisync_sql::replica::{Reach, TaskState};

use super::{Shared, common};
use crate::engine::{Engine, short_id};

#[component]
pub fn StatusView(engine: Arc<Engine>, shared: Shared) -> Element {
    let index = (shared.view_as)();
    let view = shared.views.read()[index].clone();
    let feed = shared.feeds.read()[index].clone();
    let mut feed_paused = shared.feed_paused;
    let Some(s) = view.status.clone() else {
        return rsx! { div { class: "intro", "Loading…" } };
    };
    let look = common::level_look(&engine, &s, common::files_to_read(&engine, index, &view));

    // the newest entry this device holds of each log, by its author's clock
    let newest = |device| {
        view.entries
            .iter()
            .filter(|e| e.device == device && e.entry.fork.is_none())
            .filter_map(|e| e.entry.at)
            .max()
    };

    rsx! {
        main { class: "main top-pad",
            div { class: "col-wide",
                section { class: "summary {look.class}", "aria-label": "Summary",
                    {look.icon}
                    div { style: "flex: 1 1 260px; display: flex; flex-direction: column; gap: 2px;",
                        div { class: "big", "{look.headline}" }
                        div { "{look.detail}" }
                    }
                    div { class: "counts-inline",
                        div { "to send " span { "{s.to_send}" } }
                        div { "to apply " span { "{s.to_apply}" } }
                    }
                }

                section { class: "card", "aria-label": "Devices",
                    h2 { "Devices" }
                    div { class: "scroll-x",
                        table { style: "min-width: 600px;",
                            thead { tr {
                                th { "Device" } th { "Received" } th { "Applied" } th { "Newest entry" } th { "State" }
                            } }
                            tbody {
                                for log in s.logs.iter() {
                                    {
                                        let stall = log.stall.clone().or(log.forks.iter().find_map(|f| f.stall.clone()));
                                        let row_class = if stall.is_some() { "warn" } else { "" };
                                        let newest = newest(log.device)
                                            .map(|t| common::relative(common::ts_time(t)))
                                            .unwrap_or_else(|| "–".into());
                                        let applied_style = if log.applied < log.received {
                                            "font-family: var(--mono); color: var(--warn-text); font-weight: 500;"
                                        } else {
                                            "font-family: var(--mono);"
                                        };
                                        rsx! {
                                            tr { class: "{row_class}", key: "{log.device}",
                                                td {
                                                    div { class: "who",
                                                        common::Mark { engine: engine.clone(), device: log.device }
                                                        span { style: "font-weight: 500;", "{engine.name_of(&log.device)}" }
                                                        if log.is_this_device {
                                                            span { class: "muted", style: "font-size: 12px;", "this device" }
                                                        }
                                                    }
                                                    div { class: "mono who-id", "{short_id(&log.device)}" }
                                                }
                                                td { style: "font-family: var(--mono);", "{log.received}" }
                                                td { style: "{applied_style}", "{log.applied}" }
                                                td { class: "muted", "{newest}" }
                                                td {
                                                    if let Some(st) = &stall {
                                                        {
                                                            let (short, long) = common::stall_text(st);
                                                            rsx! {
                                                                div { style: "display: flex; flex-direction: column; gap: 2px;",
                                                                    span { class: "state-warn", common::ClockIcon {} "{short}" }
                                                                    span { class: "muted", style: "font-size: 12px;",
                                                                        "{common::count(log.received - log.applied, \"entry\", \"entries\")} waiting: {long}"
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    } else {
                                                        span { class: "state-ok", common::CheckIcon {} "OK" }
                                                    }
                                                    for f in log.forks.iter() {
                                                        div { class: "muted", style: "font-size: 12px;",
                                                            "fork after entry {f.forked_at}: {f.received} received · {f.applied} applied"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if s.logs.is_empty() {
                                    tr { td { colspan: "5", class: "muted", "No entries yet." } }
                                }
                            }
                        }
                    }
                }

                section { class: "card", "aria-label": "Sync folders",
                    h2 { "Sync folders" }
                    for r in s.remotes.iter() {
                        div { class: "row-wrap", key: "{r.id}",
                            div { class: "grow",
                                span { style: "font-weight: 500;", "Local folder" }
                                span { class: "mono muted", style: "word-break: break-all;", "{r.config}" }
                            }
                            match &r.reach {
                                Reach::Ok if s.paused => rsx! { span { class: "state-warn", common::PauseIcon {} "Paused" } },
                                Reach::Ok => rsx! { span { class: "state-ok", common::CheckIcon {} "Reachable" } },
                                Reach::Unreachable { error, attempts, .. } => rsx! {
                                    span { class: "state-warn", common::WarnIcon {} "Unreachable ({attempts} tries): {error}" }
                                },
                            }
                            span { class: "muted",
                                match r.last_read_at {
                                    Some(t) => format!("read {}", common::relative(t)),
                                    None => "not read yet".into(),
                                }
                            }
                            span { class: "muted",
                                match r.last_publish_at {
                                    Some(t) => format!("wrote {}", common::relative(t)),
                                    None => "nothing written since start".into(),
                                }
                            }
                            span { class: "muted", "{r.files_read} files read · {r.blocked.len()} blocked · {r.to_send} to send" }
                        }
                        for b in r.blocked.iter() {
                            div { class: "row-wrap", style: "padding-top: 0; color: var(--warn-text);",
                                "Blocked: file {b.file} from {engine.name_of(&b.publisher)}, {b.attempts} attempts: {common::blocked_reasons(b.reasons)}"
                                if b.waiting_on_files > 0 { " · waiting on {b.waiting_on_files} earlier files" }
                                if !b.unknown_devices.is_empty() { " · {b.unknown_devices.len()} unknown devices" }
                            }
                        }
                    }
                    if s.remotes.is_empty() {
                        div { class: "empty", "No sync folder attached." }
                    }
                }

                section { class: "card row-wrap", "aria-label": "Background tasks",
                    h2 { style: "padding: 0; border: 0;", "Background tasks" }
                    for t in s.tasks.iter() {
                        span { key: "{common::task_name(t.task)}",
                            "{common::task_name(t.task)} "
                            match &t.state {
                                TaskState::Running => rsx! { span { style: "color: var(--ok-text); font-weight: 500;", "running" } },
                                TaskState::Stopped { error } => rsx! {
                                    span { style: "color: var(--warn-text); font-weight: 600;", "stopped: {error}, restarting" }
                                },
                            }
                            if t.restarts > 0 {
                                span { class: "muted", " ({t.restarts} restarts)" }
                            }
                        }
                    }
                }
            }

            aside { class: "card col-side", "aria-label": "Activity", style: "display: flex; flex-direction: column;",
                div { class: "side-head",
                    h2 { "Activity" }
                    button {
                        class: "btn small",
                        "aria-pressed": if feed_paused() { "true" } else { "false" },
                        onclick: move |_| { let p = feed_paused(); feed_paused.set(!p) },
                        if feed_paused() { "Resume feed" } else { "Pause feed" }
                    }
                }
                ol { class: "feed",
                    for (i, item) in feed.iter().enumerate() {
                        li { key: "{i}", class: if item.warn { "warn" } else { "" },
                            span { class: "time mono", "{common::clock(item.at)}" }
                            span { "{item.text}" }
                        }
                    }
                    if feed.is_empty() {
                        li { class: "muted", "Nothing yet. Edit something on either device." }
                    }
                }
            }
        }
    }
}
