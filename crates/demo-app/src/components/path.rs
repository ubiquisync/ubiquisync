use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;
use ubiquisync_core::uuid::Uuid;

use crate::state::{Drawing, Path as PathState};

use super::{fill_stroke::fill_stroke_attrs, sub_path::append_subpath_state};

/// Render one path with stable shared handles for its keyed component lifetime.
#[component]
pub fn Path(
    state: PathState,
    members: Signal<HashSet<Uuid>>,
    drawing: Drawing,
    vertex_members: Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
) -> Element {
    let d = use_memo(move || {
        let ids = members.read();
        // Membership and individual registers supply the geometry subscriptions.
        let sub_paths = drawing.sub_path.peek();
        let indexes = vertex_members.peek();

        let mut ordered = ids
            .iter()
            .filter_map(|id| {
                let sub_path = sub_paths.get(id)?;
                if *sub_path.deleted.read() {
                    return None;
                }
                Some((sub_path.sort_order.read().clone(), *id, sub_path))
            })
            .collect::<Vec<_>>();

        ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        let mut out = String::new();
        for (_, id, sub_path) in ordered {
            let vertices = indexes
                .get(&id)
                .expect("subpath membership initialized before publication");
            append_subpath_state(&mut out, sub_path, vertices, &drawing.vertex);
        }
        out
    });

    if !*state.deleted.read() {
        let attrs = fill_stroke_attrs(&state.fill_stroke);
        rsx! {
            path {
                d: "{d}",
                ..attrs
            }
        }
    } else {
        rsx! {}
    }
}
