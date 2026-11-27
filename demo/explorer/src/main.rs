//! Ubiquisync Explorer: a demo of the sync engine. Two devices run in this one window,
//! each with its own database, syncing through one local folder. Not for real data.

mod engine;
mod schema;
mod ui;

use std::sync::Arc;

use crate::engine::{Engine, demo_dir, runtime};

const CSS: &str = include_str!("../assets/style.css");

fn main() {
    let dir = demo_dir();
    let engine = match runtime().block_on(Engine::open(dir.clone(), 0)) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            eprintln!(
                "could not open the demo devices in {}: {e}\n\
                 (is another copy of the explorer running? set UBIQUISYNC_DEMO_DIR to use another directory)",
                dir.display()
            );
            std::process::exit(1);
        }
    };
    eprintln!("demo data in {}", dir.display());

    let window = dioxus::desktop::WindowBuilder::new()
        .with_title("Ubiquisync Explorer")
        .with_inner_size(dioxus::desktop::LogicalSize::new(1280.0, 860.0));
    let cfg = dioxus::desktop::Config::new()
        .with_window(window)
        .with_custom_head(format!("<style>{CSS}</style>"));

    // UBIQUISYNC_EXPLORER_SMOKE=1: run one edit through both devices without a window, then exit
    if std::env::var_os("UBIQUISYNC_EXPLORER_SMOKE").is_some() {
        match runtime().block_on(engine::smoke_test(engine)) {
            Ok(msg) => eprintln!("smoke test passed: {msg}"),
            Err(e) => {
                eprintln!("smoke test failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    dioxus::LaunchBuilder::new()
        .with_cfg(cfg)
        .with_context(engine)
        .launch(ui::App);
}
