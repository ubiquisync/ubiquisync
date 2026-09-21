use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use dioxus::{html::input_data::MouseButton, prelude::*};
use ubiquisync_core::uuid::Uuid;

use super::{
    drawing::{Drag, DragKind, Extension, Prototype},
    path::Path,
    path_geometry::Point,
    sub_path::{ResolvedVertex, render_subpath},
};
use crate::state::{Drawing, Path as PathState, XY};

#[component]
pub(super) fn EditablePath(
    id: Uuid,
    state: PathState,
    drawing: Drawing,
    members: Signal<HashSet<Uuid>>,
    vertex_members: Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
    mut selected: Signal<Option<Uuid>>,
    mut selected_vertex: Signal<Option<Uuid>>,
    drag: Signal<Option<Drag>>,
    mut extending: Signal<Option<Extension>>,
) -> Element {
    let model = use_context::<Rc<RefCell<Prototype>>>();
    if *state.deleted.read() {
        return rsx! {};
    }
    let active = *selected.read() == Some(id);
    let mut segments = Vec::new();
    let mut points = Vec::new();
    {
        let subpaths = drawing.sub_path.peek();
        let vertices = drawing.vertex.peek();
        let indexes = vertex_members.peek();
        for sub_id in members.read().iter() {
            let Some(sub) = subpaths.get(sub_id) else {
                continue;
            };
            if *sub.deleted.read() {
                continue;
            }
            let Some(ids) = indexes.get(sub_id) else {
                continue;
            };
            let mut ordered = ids
                .read()
                .iter()
                .filter_map(|id| {
                    let vertex = vertices.get(id)?;
                    if *vertex.deleted.read() {
                        return None;
                    }
                    Some((
                        vertex.sort_order.read().clone(),
                        *id,
                        ResolvedVertex {
                            pos: (*vertex.position.read())?,
                            in_handle: *vertex.incoming_handle.read(),
                            out_handle: *vertex.outgoing_handle.read(),
                        },
                    ))
                })
                .collect::<Vec<_>>();
            ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            for pair in ordered.windows(2) {
                let mut d = String::new();
                render_subpath(&mut d, [pair[0].2, pair[1].2].into_iter(), false);
                segments.push((*sub_id, pair[0].1, pair[1].1, d));
            }
            let closed = *sub.closed.read();
            if closed && let (Some(first), Some(last)) = (ordered.first(), ordered.last()) {
                let mut d = String::new();
                render_subpath(&mut d, [last.2, first.2].into_iter(), false);
                segments.push((*sub_id, last.1, first.1, d));
            }
            let len = ordered.len();
            for (index, (_, vertex_id, vertex)) in ordered.into_iter().enumerate() {
                points.push((
                    vertex_id,
                    vertex,
                    closed || index > 0,
                    closed || index + 1 < len,
                ));
            }
        }
    }
    rsx! {
        g {
            ondoubleclick: move |event| event.stop_propagation(),
            Path { state, drawing, members, vertex_members }
            for (sub, from, to, d) in segments {
                path {
                    key: "{from:?}-{to:?}",
                    d, fill: "none", stroke: "transparent", stroke_width: 16,
                    pointer_events: "stroke", style: "cursor: crosshair",
                    onpointerdown: move |event| {
                        if !event.is_primary() || event.trigger_button() != Some(MouseButton::Primary) { return; }
                        event.stop_propagation();
                        if *selected.peek() != Some(id) { selected_vertex.set(None); }
                        selected.set(Some(id));
                        extending.set(None);
                    },
                    ondoubleclick: {
                        let model = model.clone();
                        move |event: MouseEvent| {
                            event.stop_propagation();
                            let model = model.clone();
                            async move {
                                if !active || event.trigger_button() != Some(MouseButton::Primary) { return; }
                                let p = event.client_coordinates();
                                // SVG path event offsets vary with their target. Convert client
                                // coordinates using the canvas transform instead.
                                let script = format!(r#"
                                    const svg = document.getElementById('drawing-canvas');
                                    const p = new DOMPoint({}, {}).matrixTransform(svg.getScreenCTM().inverse());
                                    return [p.x, p.y];
                                "#, p.x, p.y);
                                if let Ok([x, y]) = document::eval(&script).join::<[f64; 2]>().await
                                    && let Some(vertex) = model.borrow_mut().insert_point(sub, from, to, Point(x, y))
                                {
                                    selected.set(Some(id));
                                    selected_vertex.set(Some(vertex));
                                }
                            }
                        }
                    },
                }
            }
            if active {
                for (vertex_id, vertex, incoming, outgoing) in points {
                    VertexControls {
                        key: "{vertex_id:?}", id: vertex_id, vertex, incoming, outgoing,
                        selected_vertex, drag, extending,
                    }
                }
            }
        }
    }
}

#[component]
fn VertexControls(
    id: Uuid,
    vertex: ResolvedVertex,
    incoming: bool,
    outgoing: bool,
    mut selected_vertex: Signal<Option<Uuid>>,
    mut drag: Signal<Option<Drag>>,
    mut extending: Signal<Option<Extension>>,
) -> Element {
    let active = *selected_vertex.read() == Some(id);
    let mut start = move |event: PointerEvent, kind: DragKind, value: XY| {
        if !event.is_primary() || event.trigger_button() != Some(MouseButton::Primary) {
            return;
        }
        event.stop_propagation();
        selected_vertex.set(Some(id));
        if kind != DragKind::Vertex {
            extending.set(None);
        }
        let p = event.client_coordinates();
        drag.set(Some(Drag {
            id,
            pointer_id: event.pointer_id(),
            kind,
            start: (p.x, p.y),
            position: value,
            radius: XY { x: 0, y: 0 },
            moved: false,
        }));
    };
    let handles = [
        (
            DragKind::Incoming,
            incoming,
            vertex.in_handle,
            XY { x: -40, y: 0 },
        ),
        (
            DragKind::Outgoing,
            outgoing,
            vertex.out_handle,
            XY { x: 40, y: 0 },
        ),
    ];
    rsx! {
        g {
            ondoubleclick: move |event| event.stop_propagation(),
            if active {
                for (kind, enabled, handle, fallback) in handles {
                    if enabled {
                        line {
                            x1: vertex.pos.x, y1: vertex.pos.y,
                            x2: vertex.pos.x + handle.unwrap_or(fallback).x,
                            y2: vertex.pos.y + handle.unwrap_or(fallback).y,
                            stroke: "#a855f7", stroke_width: 1,
                            stroke_dasharray: if handle.is_some() { "none" } else { "4 3" },
                            pointer_events: "none",
                        }
                        circle {
                            cx: vertex.pos.x + handle.unwrap_or(fallback).x,
                            cy: vertex.pos.y + handle.unwrap_or(fallback).y,
                            r: 7, fill: if handle.is_some() { "#a855f7" } else { "white" },
                            stroke: "#9333ea", stroke_width: 2, style: "cursor: move",
                            onpointerdown: move |event| start(event, kind, handle.unwrap_or(fallback)),
                        }
                    }
                }
            }
            if !incoming || !outgoing {
                circle { cx: vertex.pos.x, cy: vertex.pos.y, r: 10, fill: "none",
                    stroke: if extending.read().is_some_and(|e| e.endpoint == id) { "#f59e0b" } else { "#5eead4" },
                    stroke_width: 2, pointer_events: "none",
                }
            }
            circle {
                cx: vertex.pos.x, cy: vertex.pos.y, r: 7,
                fill: if active { "#0f766e" } else { "white" },
                stroke: "#0f766e", stroke_width: 2, style: "cursor: move",
                onpointerdown: move |event| start(event, DragKind::Vertex, vertex.pos),
            }
        }
    }
}
