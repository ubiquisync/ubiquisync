use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};

use dioxus::{
    signals::{ReadableExt, Signal},
    stores::Store,
};
use ubiquisync_core::uuid::Uuid;

use crate::state::{SubPath, Vertex, XY};

#[derive(Clone, Copy, PartialEq)]
pub struct ResolvedVertex {
    pub pos: XY,
    pub in_handle: Option<XY>,
    pub out_handle: Option<XY>,
}

/// Append geometry while tracking membership and the fields used to render it.
pub fn append_subpath_state(
    out: &mut String,
    state: &SubPath,
    members: &Signal<HashSet<Uuid>>,
    vertices: &Store<HashMap<Uuid, Vertex>>,
) {
    if *state.deleted.read() {
        return;
    }

    let closed = *state.closed.read();
    let mut resolved = {
        // Subscribe to changes
        let ids = members.read();
        // Avoid subscribing to all changes here!
        let objects = vertices.peek();

        ids.iter()
            .filter_map(|id| {
                let vertex = objects.get(id)?;
                if *vertex.deleted.read() {
                    return None;
                }

                let pos = (*vertex.position.read())?;
                let order = vertex.sort_order.read().clone();
                Some((
                    order,
                    *id,
                    ResolvedVertex {
                        pos,
                        in_handle: *vertex.incoming_handle.read(),
                        out_handle: *vertex.outgoing_handle.read(),
                    },
                ))
            })
            .collect::<Vec<_>>()
    };

    resolved.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    render_subpath(
        out,
        resolved.into_iter().map(|(_, _, vertex)| vertex),
        closed,
    );
}

pub(super) fn render_subpath(
    out: &mut String,
    mut vertices: impl Iterator<Item = ResolvedVertex>,
    closed: bool,
) {
    let Some(first) = vertices.next() else { return };

    if !out.is_empty() {
        out.push(' ');
    }

    write!(out, "M {} {}", first.pos.x, first.pos.y).unwrap();

    let mut prev = first;
    for next in vertices {
        append_segment(out, &prev, &next);
        prev = next;
    }

    if closed {
        append_segment(out, &prev, &first);
        write!(out, " Z").unwrap();
    }
}

fn append_segment(out: &mut String, from: &ResolvedVertex, to: &ResolvedVertex) {
    let outgoing = from.out_handle.map(|offset| add_xy(&from.pos, &offset));
    let incoming = to.in_handle.map(|offset| add_xy(&to.pos, &offset));

    let XY { x, y } = to.pos;

    match (outgoing, incoming) {
        // L: straight line
        (None, None) => write!(out, " L {x} {y}"),
        // Q: quadratic bezier
        (Some((cx, cy)), None) | (None, Some((cx, cy))) => write!(out, " Q {cx} {cy} {x} {y}"),
        // C: cubic bezier
        (Some((cx1, cy1)), Some((cx2, cy2))) => write!(out, " C {cx1} {cy1} {cx2} {cy2} {x} {y}"),
    }
    .unwrap()
}

// widen output to avoid overflow
fn add_xy(pos: &XY, offset: &XY) -> (i128, i128) {
    (
        pos.x as i128 + offset.x as i128,
        pos.y as i128 + offset.y as i128,
    )
}
