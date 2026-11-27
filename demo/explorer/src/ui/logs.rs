//! "Logs": every entry this device holds, merged by time or per device, and the history of
//! one item: every write to each of its fields, and which one is current.

use std::sync::Arc;

use dioxus::prelude::*;
use ubiquisync_core::ids::PeerId;
use ubiquisync_sql::replica::EntryKind;

use super::{Shared, common};
use crate::{
    engine::{Engine, Entry},
    schema::{column_name, show_value, table_name, todo_id, todos},
};

fn change_text(e: &Entry) -> String {
    match (&e.entry.kind, &e.op) {
        (EntryKind::Op, Some(op)) => {
            let key = todo_id(op)
                .map(|id| format!("{}…", hex::encode(&id[..2])))
                .unwrap_or_else(|| format!("{:?}", op.key));
            if op.deleted {
                return format!("{} {key} delete", table_name(op));
            }
            let sets: Vec<String> = op
                .sets
                .iter()
                .map(|(c, v)| {
                    let name = column_name(op, *c);
                    let shown = match (name.as_str(), v) {
                        ("done", Some(ubiquisync_tables::op::Value::I64(n))) => n.to_string(),
                        _ => show_value(&name, v),
                    };
                    format!("{name}={shown}")
                })
                .collect();
            format!("{} {key} set {}", table_name(op), sets.join(", "))
        }
        (EntryKind::Op, None) => "op this demo cannot decode".into(),
        (EntryKind::UseKey, _) => "switch encryption key".into(),
        (EntryKind::Expunged, _) => "expunged entry".into(),
        (EntryKind::Signature, _) => "signature".into(),
        (EntryKind::Unreadable(err), _) => format!("unreadable: {err}"),
    }
}

#[component]
pub fn LogsView(engine: Arc<Engine>, shared: Shared) -> Element {
    let index = (shared.view_as)();
    let view = shared.views.read()[index].clone();
    let mut merged = use_signal(|| true);
    let mut filter: Signal<Option<usize>> = use_signal(|| None);
    let mut selected = shared.selected;
    let sel = selected();

    let mut rows: Vec<Entry> = view
        .entries
        .iter()
        .filter(|e| match filter() {
            Some(i) => engine.index_of(&e.device) == Some(i),
            None => true,
        })
        .cloned()
        .collect();
    if merged() {
        rows.sort_by(|a, b| {
            b.entry
                .at
                .cmp(&a.entry.at)
                .then(b.entry.index.cmp(&a.entry.index))
        });
    } else {
        rows.sort_by(|a, b| {
            let ia = engine.index_of(&a.device).unwrap_or(usize::MAX);
            let ib = engine.index_of(&b.device).unwrap_or(usize::MAX);
            ia.cmp(&ib)
                .then(a.entry.fork.cmp(&b.entry.fork))
                .then(b.entry.index.cmp(&a.entry.index))
        });
    }

    rsx! {
        main { class: "main top-pad",
            section { class: "card col-wide", style: "gap: 0; overflow: hidden;", "aria-label": "Entries",
                div { class: "toolbar",
                    h2 { "Entries" }
                    div { class: "seg", role: "group", "aria-label": "Order",
                        button { "aria-pressed": if merged() { "true" } else { "false" }, onclick: move |_| merged.set(true), "Merged by time" }
                        button { "aria-pressed": if merged() { "false" } else { "true" }, onclick: move |_| merged.set(false), "Per device" }
                    }
                    select {
                        "aria-label": "Filter by device",
                        onchange: move |e| filter.set(e.value().parse().ok()),
                        option { value: "all", selected: filter().is_none(), "All devices" }
                        for (i, d) in engine.devices.iter().enumerate() {
                            option { value: "{i}", selected: filter() == Some(i), "Device {d.name}" }
                        }
                    }
                }
                div { class: "scroll-x",
                    table { class: "entries", style: "min-width: 640px;",
                        thead { tr {
                            th { "Device" } th { "#" } th { "Time" } th { "Change" } th { "Hash" }
                        } }
                        tbody {
                            for e in rows.iter() {
                                {
                                    let id = e.op.as_ref().and_then(todo_id);
                                    let mut class = String::new();
                                    if sel.is_some() && id == sel {
                                        class.push_str(&format!("sel-{}", common::device_class(&engine, &e.device)));
                                    }
                                    if !e.entry.applied {
                                        class.push_str(" pending");
                                    }
                                    let time = e.entry.at.map(|t| common::clock_ms(common::ts_time(t))).unwrap_or_default();
                                    let number = match e.entry.fork {
                                        Some(f) => format!("{} (fork after {f})", e.entry.index),
                                        None => e.entry.index.to_string(),
                                    };
                                    let hash = hex::encode(&e.entry.chain_hash[..3]);
                                    let change = change_text(e);
                                    rsx! {
                                        tr {
                                            key: "{e.device}-{e.entry.fork:?}-{e.entry.index}",
                                            class: "{class}",
                                            title: if e.entry.applied { "Click to see this item's history" } else { "Received but not applied yet" },
                                            onclick: move |_| { if id.is_some() { selected.set(id) } },
                                            td { span { class: "dev",
                                                common::Mark { engine: engine.clone(), device: e.device }
                                                "{common::letter(&engine, &e.device)}"
                                            } }
                                            td { "{number}" }
                                            td { "{time}" }
                                            td { "{change}" if !e.entry.applied { " (not applied)" } }
                                            td { class: "muted", "{hash}…" }
                                        }
                                    }
                                }
                            }
                            if rows.is_empty() {
                                tr { td { colspan: "5", class: "muted", "No entries yet." } }
                            }
                        }
                    }
                }
                div { class: "foot-note",
                    "Highlighted rows are the writes to the selected item. Each entry is signed by its device and chained to the one before it. This is what Device {engine.devices[index].name} holds."
                }
            }
            History { engine: engine.clone(), shared }
        }
    }
}

/// One write to one field of the selected item.
#[derive(Clone)]
struct Write {
    device: PeerId,
    index: u64,
    at: Option<ubiquisync_core::hlc::Timestamp>,
    value: String,
    applied: bool,
}

#[component]
fn History(engine: Arc<Engine>, shared: Shared) -> Element {
    let index = (shared.view_as)();
    let view = shared.views.read()[index].clone();
    let Some(id) = (shared.selected)() else {
        return rsx! {
            aside { class: "card col-side", "aria-label": "History of one item",
                div { class: "history-head", h2 { "Item history" } }
                div { class: "empty", "Select a row in the table, or use \"history\" on an item, to see every write to it." }
            }
        };
    };

    let ops: Vec<&Entry> = view
        .entries
        .iter()
        .filter(|e| e.op.as_ref().and_then(todo_id) == Some(id))
        .collect();
    let columns: Vec<String> = todos::table()
        .map(|t| t.columns().map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    let mut fields: Vec<(String, Vec<Write>)> = vec![];
    for name in &columns {
        let mut writes: Vec<Write> = vec![];
        for e in &ops {
            let op = e.op.as_ref().unwrap();
            for (c, v) in &op.sets {
                if &column_name(op, *c) == name {
                    writes.push(Write {
                        device: e.device,
                        index: e.entry.index,
                        at: e.entry.at,
                        value: show_value(name, v).trim_matches('"').to_string(),
                        applied: e.entry.applied,
                    });
                }
            }
        }
        // last writer wins: the latest clock is current
        writes.sort_by_key(|w| std::cmp::Reverse(w.at));
        if !writes.is_empty() {
            fields.push((name.clone(), writes));
        }
    }
    let deletes: Vec<&Entry> = ops
        .iter()
        .copied()
        .filter(|e| e.op.as_ref().is_some_and(|o| o.deleted))
        .collect();
    let winners: Vec<PeerId> = fields
        .iter()
        .filter(|(n, _)| n != "created_at")
        .filter_map(|(_, w)| w.iter().find(|w| w.applied).map(|w| w.device))
        .collect();
    let mixed = winners.windows(2).any(|w| w[0] != w[1]);
    let why = if mixed {
        "Each field keeps the write with the latest time. Different devices last changed different fields, so neither replaced the other's change."
    } else {
        "Each field keeps the write with the latest time, whichever device it came from."
    };

    rsx! {
        aside { class: "card col-side", "aria-label": "History of one item",
            div { class: "history-head",
                h2 { "Item history" }
                div { class: "mono muted", "todos · {hex::encode(&id[..2])} {hex::encode(&id[2..4])} … {hex::encode(&id[14..])}" }
            }
            for (name, writes) in fields.iter() {
                div { class: "field", key: "{name}",
                    div { class: "field-name", "{name}" }
                    for (i, w) in writes.iter().enumerate() {
                        {
                            let current = i == 0 && w.applied;
                            let class = if current { "write current" } else if w.applied { "write replaced" } else { "write" };
                            let tag = if current { "current" } else if w.applied { "replaced" } else { "not applied yet" };
                            let when = w.at.map(|t| common::clock_ms(common::ts_time(t))).unwrap_or_default();
                            rsx! {
                                div { class: "{class}", key: "{w.device}-{w.index}",
                                    common::Mark { engine: engine.clone(), device: w.device }
                                    div { style: "flex: 1; min-width: 0;",
                                        div { class: "val", "{w.value}" }
                                        div { class: "mono muted", "{common::letter(&engine, &w.device)} #{w.index} · {when}" }
                                    }
                                    span { class: "tag", "{tag}" }
                                }
                            }
                        }
                    }
                }
            }
            if !deletes.is_empty() {
                div { class: "field",
                    div { class: "field-name", "deleted" }
                    for d in deletes.iter() {
                        div { class: "write",
                            common::Mark { engine: engine.clone(), device: d.device }
                            div { style: "flex: 1;",
                                div { class: "val", "deleted" }
                                div { class: "mono muted",
                                    "{common::letter(&engine, &d.device)} #{d.entry.index} · {d.entry.at.map(|t| common::clock_ms(common::ts_time(t))).unwrap_or_default()}"
                                }
                            }
                        }
                    }
                }
            }
            if fields.is_empty() && deletes.is_empty() {
                div { class: "empty", "This device holds no writes to this item." }
            }
            div { class: "why",
                div { b { "Why this result" } }
                div { "{why}" }
            }
        }
    }
}
