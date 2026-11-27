//! The window: tabs, shared state, and the per-device refresh loops.

mod common;
mod devices;
mod logs;
mod status;

use std::{
    pin::pin,
    sync::Arc,
    time::{Duration, SystemTime},
};

use dioxus::prelude::*;
use futures::{
    FutureExt, StreamExt,
    future::{Either, select},
};
use ubiquisync_sql::replica::{SyncEvent, SyncEventKind};

use crate::engine::{DeviceView, Engine, on_engine, sleep};
use common::stall_text;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Devices,
    Status,
    Logs,
}

/// One line of the activity feed.
#[derive(Clone, PartialEq)]
pub struct FeedItem {
    pub at: SystemTime,
    pub text: String,
    pub warn: bool,
}

const FEED_LEN: usize = 200;

/// State shared by the views of one engine generation.
#[derive(Clone, Copy, PartialEq)]
pub struct Shared {
    pub views: Signal<Vec<DeviceView>>,
    pub feeds: Signal<Vec<Vec<FeedItem>>>,
    pub feed_paused: Signal<bool>,
    pub tab: Signal<Tab>,
    /// Which device the Status and Logs views look through.
    pub view_as: Signal<usize>,
    /// The todo whose history the Logs view shows.
    pub selected: Signal<Option<[u8; 16]>>,
}

impl Shared {
    /// Reloads one device's view now, instead of waiting for its next refresh.
    pub fn reload(self, engine: Arc<Engine>, index: usize) {
        let mut views = self.views;
        spawn(async move {
            let v = DeviceView::load(engine, index).await;
            views.write()[index] = v;
        });
    }
}

#[component]
pub fn App() -> Element {
    let initial: Arc<Engine> = use_context();
    let slot: Signal<Option<Arc<Engine>>> = use_signal(|| Some(initial));
    let error: Signal<Option<String>> = use_signal(|| None);
    let current = slot();
    rsx! {
        if let Some(e) = error() {
            div { class: "error", "Reset failed: {e}" }
        }
        match current {
            Some(engine) => rsx! {
                for e in [engine] {
                    Session { key: "{e.generation}", engine: e, slot, error }
                }
            },
            None => rsx! {
                div { class: "intro", "Resetting demo data…" }
            },
        }
    }
}

#[component]
fn Session(
    engine: Arc<Engine>,
    slot: Signal<Option<Arc<Engine>>>,
    error: Signal<Option<String>>,
) -> Element {
    let shared = Shared {
        views: use_signal(|| vec![DeviceView::default(); 2]),
        feeds: use_signal(|| vec![vec![]; 2]),
        feed_paused: use_signal(|| false),
        tab: use_signal(|| Tab::Devices),
        view_as: use_signal(|| 0),
        selected: use_signal(|| None),
    };
    use_device_loop(engine.clone(), 0, shared);
    use_device_loop(engine.clone(), 1, shared);

    let mut tab = shared.tab;
    let tab_button = move |t: Tab, label: &'static str| {
        rsx! {
            button {
                class: "tab",
                "aria-current": if tab() == t { "page" } else { "false" },
                onclick: move |_| tab.set(t),
                "{label}"
            }
        }
    };

    let reset = {
        let engine = engine.clone();
        move |_| {
            let mut slot = slot;
            let mut error = error;
            let engine = engine.clone();
            // not `spawn`: this component goes away as soon as `slot` is cleared
            dioxus::core::spawn_forever(async move {
                slot.set(None);
                // let the old views' tasks drop their handles before shutting down
                sleep(Duration::from_millis(300)).await;
                let res = on_engine(async move {
                    let owned = wait_for_sole_owner(engine).await?;
                    owned.reset().await
                })
                .await;
                match res {
                    Ok(e) => {
                        error.set(None);
                        slot.set(Some(Arc::new(e)));
                    }
                    Err(e) => error.set(Some(e)),
                }
            });
        }
    };

    rsx! {
        div { class: "app",
            header { class: "top",
                div { class: "brand", "Ubiquisync Explorer" }
                nav { class: "tabs", "aria-label": "Views",
                    {tab_button(Tab::Devices, "Two devices")}
                    {tab_button(Tab::Status, "Status")}
                    {tab_button(Tab::Logs, "Logs")}
                }
                div { class: "top-right",
                    if tab() != Tab::Devices {
                        common::ViewAs { engine: engine.clone(), shared }
                    } else {
                        span { "A demo of the sync engine. Not for real data." }
                    }
                    button {
                        class: "btn small",
                        title: "Shut both devices down, delete their databases and the sync folder, and start again",
                        onclick: reset,
                        "Reset demo data"
                    }
                }
            }
            match tab() {
                Tab::Devices => rsx! { devices::TwoDevices { engine: engine.clone(), shared } },
                Tab::Status => rsx! { status::StatusView { engine: engine.clone(), shared } },
                Tab::Logs => rsx! { logs::LogsView { engine: engine.clone(), shared } },
            }
        }
    }
}

/// Waits (up to 5 s) until nothing else holds the engine, so it can be shut down.
async fn wait_for_sole_owner(mut engine: Arc<Engine>) -> Result<Engine, String> {
    let started = std::time::Instant::now();
    loop {
        match Arc::try_unwrap(engine) {
            Ok(e) => return Ok(e),
            Err(e) if started.elapsed() > Duration::from_secs(5) => {
                return Err(format!(
                    "the old devices are still in use ({} handles)",
                    Arc::strong_count(&e)
                ));
            }
            Err(e) => engine = e,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Keeps one device's view fresh: reloads it after every burst of sync events, and at
/// least once a second so relative times stay current. Events go to the device's feed.
fn use_device_loop(engine: Arc<Engine>, index: usize, shared: Shared) {
    use_future(move || {
        let engine = engine.clone();
        let mut views = shared.views;
        let mut feeds = shared.feeds;
        let feed_paused = shared.feed_paused;
        async move {
            let mut events = engine.devices[index].store.watch();
            loop {
                let v = DeviceView::load(engine.clone(), index).await;
                views.write()[index] = v;

                let mut batch = vec![];
                {
                    let next = pin!(events.next());
                    let timer = pin!(sleep(Duration::from_secs(1)));
                    if let Either::Left((Some(ev), _)) = select(next, timer).await {
                        batch.push(ev);
                    }
                }
                if !batch.is_empty() {
                    while let Some(Some(ev)) = events.next().now_or_never() {
                        batch.push(ev);
                    }
                    if !feed_paused() {
                        let mut feeds = feeds.write();
                        let feed = &mut feeds[index];
                        for ev in batch {
                            feed.insert(0, feed_item(&engine, index, &ev));
                        }
                        feed.truncate(FEED_LEN);
                    }
                    // let a burst of writes settle before reading again
                    sleep(Duration::from_millis(80)).await;
                }
            }
        }
    });
}

fn feed_item(engine: &Engine, index: usize, ev: &SyncEvent) -> FeedItem {
    let me = engine.devices[index].id;
    let name = |p| engine.name_of(p);
    let n_entries = |n: u64| common::count(n, "entry", "entries");
    let (text, warn) = match &ev.kind {
        SyncEventKind::FilePublished { entries, bytes, .. } => (
            format!(
                "Wrote a file: {}, {}",
                n_entries(*entries),
                common::size(*bytes)
            ),
            false,
        ),
        SyncEventKind::FileRead {
            publisher, entries, ..
        } => (
            format!(
                "Read a file from {}: {}",
                if *publisher == me {
                    "this device".into()
                } else {
                    name(publisher)
                },
                n_entries(*entries)
            ),
            false,
        ),
        SyncEventKind::FileBlocked {
            publisher, reasons, ..
        } => (
            format!(
                "A file from {} could not be read yet ({})",
                name(publisher),
                common::blocked_reasons(*reasons)
            ),
            true,
        ),
        SyncEventKind::Applied {
            device, from, to, ..
        } => {
            let n = n_entries(to - from);
            if *device == me {
                (format!("Wrote {n} on this device"), false)
            } else {
                (format!("Applied {n} from {}", name(device)), false)
            }
        }
        SyncEventKind::Stalled { device, stall, .. } => (
            format!("{} stalled: {}", name(device), stall_text(stall).0),
            true,
        ),
        SyncEventKind::Recovered { device, .. } => {
            (format!("{} applies entries again", name(device)), false)
        }
        SyncEventKind::Forked { device, at, .. } => (
            format!("{}'s log forked after entry {at}", name(device)),
            true,
        ),
        SyncEventKind::DeviceSeen { device } => {
            (format!("New device seen: {}", name(device)), false)
        }
        SyncEventKind::RemoteLost { error, .. } => {
            (format!("Sync folder unreachable: {error}"), true)
        }
        SyncEventKind::RemoteBack { .. } => ("Sync folder reachable again".into(), false),
        SyncEventKind::TaskStopped { task, error } => (
            format!("{} stopped: {error}", common::task_name(*task)),
            true,
        ),
        SyncEventKind::Missed(n) => (format!("Missed {n} events (feed fell behind)"), true),
    };
    FeedItem {
        at: ev.at,
        text,
        warn,
    }
}
