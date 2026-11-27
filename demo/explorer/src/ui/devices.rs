//! "Two devices": both todo lists side by side with the sync folder between them.

use std::{collections::HashMap, sync::Arc};

use dioxus::prelude::*;
use ubiquisync_core::ids::PeerId;

use super::{Shared, Tab, common};
use crate::{
    engine::{DeviceView, Engine, Todo, exec, new_todo_id, now_ms},
    schema::{column_name, todo_id, todos},
};

#[component]
pub fn TwoDevices(engine: Arc<Engine>, shared: Shared) -> Element {
    rsx! {
        div { class: "intro",
            "Two copies of the app share one folder. Pause either side, edit the same item on both, then resume and watch the merge."
        }
        main { class: "main",
            DevicePanel { engine: engine.clone(), shared, index: 0 }
            FolderPanel { engine: engine.clone(), shared }
            DevicePanel { engine: engine.clone(), shared, index: 1 }
        }
    }
}

/// What this device changed that is not in the sync folder yet, per todo.
fn unsent_notes(engine: &Engine, index: usize, view: &DeviceView) -> HashMap<[u8; 16], String> {
    let me = engine.devices[index].id;
    let Some(status) = &view.status else {
        return HashMap::new();
    };
    let published = status
        .logs
        .iter()
        .find(|l| l.is_this_device)
        .map(|l| l.in_every_remote)
        .unwrap_or(0);
    let mut notes: HashMap<[u8; 16], (bool, bool, bool)> = HashMap::new();
    for e in view
        .entries
        .iter()
        .filter(|e| e.device == me && e.entry.fork.is_none() && e.entry.index >= published)
    {
        let Some(op) = &e.op else { continue };
        let Some(id) = todo_id(op) else { continue };
        let n = notes.entry(id).or_default();
        for (col, _) in &op.sets {
            match column_name(op, *col).as_str() {
                "created_at" => n.0 = true,
                "title" => n.1 = true,
                "done" => n.2 = true,
                _ => {}
            }
        }
    }
    notes
        .into_iter()
        .map(|(id, (added, renamed, ticked))| {
            let text = if added {
                "added here".to_string()
            } else {
                let mut parts = vec![];
                if renamed {
                    parts.push("renamed");
                }
                if ticked {
                    parts.push("ticked");
                }
                format!("{} here", parts.join(", "))
            };
            (id, text)
        })
        .collect()
}

#[component]
fn DevicePanel(engine: Arc<Engine>, shared: Shared, index: usize) -> Element {
    let views = shared.views.read();
    let view = views[index].clone();
    drop(views);
    let device = &engine.devices[index];
    let class = if index == 0 { "a" } else { "b" };
    let mut new_title = use_signal(String::new);
    let paused = view.status.as_ref().is_some_and(|s| s.paused);

    let toggle = {
        let engine = engine.clone();
        move |_| {
            let store = &engine.devices[index].store;
            store.set_sync_paused(!store.sync_paused());
            shared.reload(engine.clone(), index);
        }
    };
    let add = {
        let engine = engine.clone();
        move || {
            let title = new_title().trim().to_string();
            if title.is_empty() {
                return;
            }
            new_title.set(String::new());
            let engine = engine.clone();
            spawn(async move {
                let id = new_todo_id();
                let op = todos::upsert(&id)
                    .title(&title)
                    .done(0)
                    .created_at(now_ms())
                    .build();
                if let Err(e) = exec(engine.clone(), index, op).await {
                    eprintln!("add failed: {e}");
                }
                shared.reload(engine, index);
            });
        }
    };

    let line = view
        .status
        .as_ref()
        .map(|s| common::level_look(&engine, s, common::files_to_read(&engine, index, &view)));
    let notes = unsent_notes(&engine, index, &view);
    let other = 1 - index;
    let counts = view.status.as_ref().map(|s| {
        let mine = s.logs.iter().find(|l| l.is_this_device);
        let theirs = s.logs.iter().find(|l| l.device == engine.devices[other].id);
        (
            mine.map(|l| (l.received, l.in_every_remote))
                .unwrap_or((0, 0)),
            theirs.map(|l| (l.received, l.applied)).unwrap_or((0, 0)),
        )
    });
    let other_name = engine.devices[other].name;

    rsx! {
        section { class: "card device {class}", "aria-label": "Device {device.name}",
            div { class: "device-head",
                div { style: "flex: 1; display: flex; flex-direction: column; gap: 2px;",
                    div { class: "device-name", "Device {device.name}" }
                    div { class: "mono muted", "{crate::engine::short_id(&device.id)}" }
                }
                button {
                    class: if paused { "sync-btn off" } else { "sync-btn on" },
                    "aria-pressed": if paused { "false" } else { "true" },
                    title: "Pause or resume reading and writing the sync folder on this device",
                    onclick: toggle,
                    if paused { "Paused · Resume" } else { "Sync on" }
                }
            }
            if let Some(l) = line {
                div { class: "line {l.class}", title: "{l.detail}", {l.icon} "{l.headline}" }
            } else if let Some(e) = &view.error {
                div { class: "line lvl-warn", "Error: {e}" }
            } else {
                div { class: "line lvl-none", "Loading…" }
            }
            div { class: "todos",
                for t in view.todos.iter().cloned() {
                    TodoRow {
                        key: "{hex::encode(t.id)}",
                        engine: engine.clone(),
                        shared,
                        index,
                        note: notes.get(&t.id).cloned(),
                        todo: t,
                    }
                }
                if view.todos.is_empty() {
                    div { class: "muted", style: "padding: 8px 0; font-size: 13px;", "No items yet." }
                }
                label { class: "add",
                    input {
                        r#type: "text",
                        "aria-label": "New item on device {device.name}",
                        placeholder: "Add an item",
                        value: "{new_title}",
                        oninput: move |e| new_title.set(e.value()),
                        onkeydown: {
                            let mut add = add.clone();
                            move |e: KeyboardEvent| {
                                if e.key() == Key::Enter {
                                    add();
                                }
                            }
                        },
                    }
                }
            }
            if let Some(((written, published), (received, applied))) = counts {
                div { class: "counts mono",
                    div { "own log   {written} written · {published} published" }
                    div { "from {other_name}    {received} received · {applied} applied" }
                }
            }
        }
    }
}

#[component]
fn TodoRow(
    engine: Arc<Engine>,
    shared: Shared,
    index: usize,
    todo: Todo,
    note: Option<String>,
) -> Element {
    let mut editing = use_signal(|| None::<String>);
    let class = if index == 0 { "a" } else { "b" };
    let id = todo.id;

    let run = {
        let engine = engine.clone();
        move |op| {
            let engine = engine.clone();
            spawn(async move {
                if let Err(e) = exec(engine.clone(), index, op).await {
                    eprintln!("write failed: {e}");
                }
                shared.reload(engine, index);
            });
        }
    };
    let save = {
        let run = run.clone();
        let old_title = todo.title.clone();
        move || {
            if let Some(title) = editing() {
                let title = title.trim().to_string();
                if !title.is_empty() && title != old_title {
                    run(todos::upsert(&id).title(&title).build());
                }
            }
            editing.set(None);
        }
    };
    let mut tab = shared.tab;
    let mut selected = shared.selected;
    let mut view_as = shared.view_as;

    let title_class = format!(
        "title{}{}",
        if todo.done { " done" } else { "" },
        if note.is_some() {
            format!(" local-{class}")
        } else {
            String::new()
        }
    );

    rsx! {
        div { class: "todo",
            input {
                r#type: "checkbox",
                checked: todo.done,
                "aria-label": "Done: {todo.title}",
                onchange: {
                    let run = run.clone();
                    move |e: FormEvent| run(todos::upsert(&id).done(if e.checked() { 1 } else { 0 }).build())
                },
            }
            if let Some(text) = editing() {
                input {
                    class: "edit",
                    r#type: "text",
                    "aria-label": "Rename item",
                    value: "{text}",
                    autofocus: true,
                    oninput: move |e| editing.set(Some(e.value())),
                    onkeydown: {
                        let mut save = save.clone();
                        move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                save();
                            } else if e.key() == Key::Escape {
                                editing.set(None);
                            }
                        }
                    },
                    onblur: {
                        let mut save = save.clone();
                        move |_| save()
                    },
                }
            } else {
                span {
                    class: "{title_class}",
                    title: "Click to rename",
                    onclick: {
                        let t = todo.title.clone();
                        move |_| editing.set(Some(t.clone()))
                    },
                    "{todo.title}"
                }
                if let Some(n) = &note {
                    span { class: "note {class}", "{n}" }
                } else {
                    span { class: "spacer" }
                }
            }
            button {
                class: "icon-btn",
                title: "See this item's history",
                "aria-label": "History of {todo.title}",
                onclick: move |_| {
                    selected.set(Some(id));
                    view_as.set(index);
                    tab.set(Tab::Logs);
                },
                "history"
            }
            button {
                class: "icon-btn",
                title: "Delete",
                "aria-label": "Delete {todo.title}",
                onclick: {
                    let run = run.clone();
                    move |_| run(todos::delete(&id))
                },
                "×"
            }
        }
    }
}

#[component]
fn FolderPanel(engine: Arc<Engine>, shared: Shared) -> Element {
    let views = shared.views.read().clone();
    let folder = engine.folder.display().to_string();
    // the listing as device A sees it; whether a file is new is up to the other device
    let files = if views[0].files.is_empty() {
        &views[1].files
    } else {
        &views[0].files
    };
    let reader_of =
        |publisher: &PeerId| -> Option<usize> { engine.index_of(publisher).map(|i| 1 - i) };
    let open_folder = {
        let folder = engine.folder.clone();
        move |_| {
            let cmd = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(target_os = "windows") {
                "explorer"
            } else {
                "xdg-open"
            };
            let _ = std::process::Command::new(cmd).arg(&folder).spawn();
        }
    };

    rsx! {
        section { class: "card folder", "aria-label": "Sync folder",
            div { class: "folder-head",
                div { style: "font-weight: 600;", "Sync folder" }
                div { class: "mono muted", style: "word-break: break-all;", "{folder}" }
            }
            div { class: "files mono",
                div { class: "files-label", "Files" }
                if files.is_empty() {
                    div { class: "muted", "Empty. Add an item on either side." }
                }
                for f in files.iter() {
                    {
                        let own = f
                            .logs
                            .iter()
                            .filter(|(log, _)| log.peer_id == f.publisher)
                            .map(|(_, r)| r.clone())
                            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end));
                        let relayed = f.logs.iter().any(|(log, _)| log.peer_id != f.publisher);
                        let what = match own {
                            Some(r) if r.end == r.start + 1 => format!("entry {}", r.start),
                            Some(r) => format!("entries {}–{}", r.start, r.end - 1),
                            None if f.header_ok => "no own entries".to_string(),
                            None => "unreadable header".to_string(),
                        };
                        let new = reader_of(&f.publisher).is_some_and(|r| {
                            !views[r].files.iter().any(|g| g.file == f.file && g.read)
                        });
                        let dc = common::device_class(&engine, &f.publisher);
                        let row_class = if new { format!("file new-{dc}") } else { "file".to_string() };
                        let letter = common::letter(&engine, &f.publisher);
                        rsx! {
                            div { class: "{row_class}", key: "{f.publisher}-{f.file}",
                                common::Mark { engine: engine.clone(), device: f.publisher }
                                span { class: "what",
                                    "{letter} · {what}"
                                    if relayed { " + relayed" }
                                }
                                if new {
                                    span { class: "new", style: if dc == "a" { "color: var(--a)" } else { "color: var(--b)" }, "new" }
                                } else {
                                    span { class: "muted", "{common::size(f.bytes)}" }
                                }
                            }
                        }
                    }
                }
                for (i, d) in engine.devices.iter().enumerate() {
                    {
                        let unread = common::files_to_read(&engine, i, &views[i]);
                        let to_send = views[i].status.as_ref().map(|s| s.to_send).unwrap_or(0);
                        let class = if i == 0 { "callout a" } else { "callout b" };
                        rsx! {
                            if unread > 0 || to_send > 0 {
                                div { class: "{class}",
                                    if unread > 0 {
                                        div { "{d.name} has not read {common::count(unread as u64, \"file\", \"files\")}" }
                                    }
                                    if to_send > 0 {
                                        div { "{d.name} has {common::count(to_send, \"entry\", \"entries\")} to send" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "folder-foot",
                button { class: "btn", onclick: open_folder, "Open in file manager" }
            }
        }
    }
}
