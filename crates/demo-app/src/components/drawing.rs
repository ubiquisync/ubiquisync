use std::{cell::RefCell, rc::Rc};

use dioxus::dioxus_core::{Runtime, current_scope_id};
use dioxus::html::input_data::MouseButton;
use dioxus::prelude::*;
use loro_fractional_index::FractionalIndex;
use ubiquisync_core::{
    hlc::{Hlc, wall_ms},
    uuid::Uuid,
};

use crate::state::{
    DrawingOp, DrawingState, Ellipse as EllipseState, EllipseOp, FillStrokeOp, Init, PathOp,
    SubPathOp, VertexOp, XY, lww::Apply,
};

use super::ellipse::Ellipse;
use super::{editable_path::EditablePath, path_geometry, sub_path::ResolvedVertex};

const INITIAL_RADIUS: i64 = 60;
const MIN_RADIUS: i64 = 8;

// Temporary local editing session; every edit still goes through drawing operations.
pub(super) struct Prototype {
    pub(super) state: DrawingState,
    owner: ScopeId,
    clock: Hlc,
    next_id: u64,
}

impl Prototype {
    fn new() -> Self {
        Self {
            state: DrawingState::init(),
            owner: current_scope_id(),
            clock: Hlc::new(0),
            next_id: 0,
        }
    }

    pub(super) fn apply(&mut self, ops: Vec<DrawingOp>) {
        let ts = self.clock.tick(wall_ms());
        // Edits may originate in child components; new signals must still live
        // as long as the drawing, rather than as long as an editing overlay.
        Runtime::current().in_scope(self.owner, || {
            for op in ops {
                self.state.apply(ts, op);
            }
        });
    }

    fn edit(&mut self, id: Uuid, ops: Vec<EllipseOp>) {
        self.apply(vec![DrawingOp::Ellipse((id, ops))]);
    }

    fn id(&mut self) -> Uuid {
        self.next_id += 1;
        // Local prototype IDs; persistence and multi-device creation are not wired yet.
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.clock.tick(wall_ms()).raw().to_be_bytes());
        id[8..].copy_from_slice(&self.next_id.to_be_bytes());
        id
    }

    fn add_circle(&mut self, position: XY) -> Uuid {
        let id = self.id();
        self.edit(
            id,
            vec![
                EllipseOp::Position(Some(position)),
                EllipseOp::Radius(Some(XY {
                    x: INITIAL_RADIUS,
                    y: INITIAL_RADIUS,
                })),
                EllipseOp::FillStroke(FillStrokeOp::Fill("#bfdbfe".into())),
                EllipseOp::FillStroke(FillStrokeOp::FillOpacity("0.7".into())),
                EllipseOp::FillStroke(FillStrokeOp::Stroke("#2563eb".into())),
                EllipseOp::FillStroke(FillStrokeOp::StrokeWidth("2".into())),
                EllipseOp::FillStroke(FillStrokeOp::StrokeOpacity("1".into())),
            ],
        );
        id
    }

    fn add_path(&mut self, position: XY) -> Uuid {
        let path = self.id();
        let sub = self.id();
        let a = self.id();
        let b = self.id();
        let order = FractionalIndex::default();
        self.apply(vec![
            DrawingOp::Path((
                path,
                vec![
                    PathOp::FillStroke(FillStrokeOp::Fill("none".into())),
                    PathOp::FillStroke(FillStrokeOp::FillOpacity("1".into())),
                    PathOp::FillStroke(FillStrokeOp::Stroke("#0f766e".into())),
                    PathOp::FillStroke(FillStrokeOp::StrokeWidth("3".into())),
                    PathOp::FillStroke(FillStrokeOp::StrokeOpacity("1".into())),
                ],
            )),
            DrawingOp::SubPath((
                sub,
                vec![
                    SubPathOp::Parent(path),
                    SubPathOp::SortOrder(order.as_bytes().to_vec()),
                ],
            )),
            DrawingOp::Vertex((
                a,
                vec![
                    VertexOp::Parent(sub),
                    VertexOp::Position(Some(XY {
                        x: position.x - 60,
                        y: position.y,
                    })),
                    VertexOp::SortOrder(order.as_bytes().to_vec()),
                ],
            )),
            DrawingOp::Vertex((
                b,
                vec![
                    VertexOp::Parent(sub),
                    VertexOp::Position(Some(XY {
                        x: position.x + 60,
                        y: position.y,
                    })),
                    VertexOp::SortOrder(FractionalIndex::new_after(&order).as_bytes().to_vec()),
                ],
            )),
        ]);
        path
    }

    pub(super) fn ordered_vertices(
        &self,
        sub: Uuid,
    ) -> Vec<(FractionalIndex, Uuid, ResolvedVertex)> {
        let vertices = self.state.drawing.vertex.peek();
        let indexes = self.state.sub_path_vertices.peek();
        let Some(ids) = indexes.get(&sub) else {
            return Vec::new();
        };
        let mut ordered = ids
            .peek()
            .iter()
            .filter_map(|id| {
                let v = vertices.get(id)?;
                if *v.deleted.peek() || *v.parent.peek() != sub {
                    return None;
                }
                Some((
                    v.sort_order.read().clone(),
                    *id,
                    ResolvedVertex {
                        pos: (*v.position.peek())?,
                        in_handle: *v.incoming_handle.peek(),
                        out_handle: *v.outgoing_handle.peek(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        ordered.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        ordered
    }

    // Return the subpath, whether this is its first vertex, and the opposite end.
    pub(super) fn endpoint(&self, id: Uuid) -> Option<(Uuid, bool, Uuid)> {
        let sub = *self.state.drawing.vertex.peek().get(&id)?.parent.peek();
        let subs = self.state.drawing.sub_path.peek();
        let state = subs.get(&sub)?;
        if *state.closed.peek() || *state.deleted.peek() {
            return None;
        }
        let ordered = self.ordered_vertices(sub);
        let first = ordered.first()?.1;
        let last = ordered.last()?.1;
        if first == last {
            return None;
        }
        if id == first {
            Some((sub, true, last))
        } else if id == last {
            Some((sub, false, first))
        } else {
            None
        }
    }

    fn extend(&mut self, endpoint: Uuid, position: XY) -> Option<Uuid> {
        let (sub, first, _) = self.endpoint(endpoint)?;
        let ordered = self.ordered_vertices(sub);
        let end = if first {
            ordered.first()?
        } else {
            ordered.last()?
        };
        if end.2.pos == position {
            return None;
        }
        let order = if first {
            FractionalIndex::new_before(&end.0)
        } else {
            FractionalIndex::new_after(&end.0)
        };
        let id = self.id();
        self.apply(vec![DrawingOp::Vertex((
            id,
            vec![
                VertexOp::Parent(sub),
                VertexOp::Position(Some(position)),
                VertexOp::SortOrder(order.as_bytes().to_vec()),
            ],
        ))]);
        Some(id)
    }

    fn close(&mut self, endpoint: Uuid, other: Uuid) -> bool {
        let Some((sub, _, opposite)) = self.endpoint(endpoint) else {
            return false;
        };
        if opposite != other {
            return false;
        }
        self.apply(vec![DrawingOp::SubPath((
            sub,
            vec![SubPathOp::Closed(true)],
        ))]);
        true
    }

    fn snap_target(&self, id: Uuid, position: XY) -> Option<(Uuid, XY)> {
        let (sub, _, other) = self.endpoint(id)?;
        // Merging a two-point line would leave just one vertex.
        if self.ordered_vertices(sub).len() < 3 {
            return None;
        }
        let target = (*self
            .state
            .drawing
            .vertex
            .peek()
            .get(&other)?
            .position
            .peek())?;
        ((position.x as f64 - target.x as f64).hypot(position.y as f64 - target.y as f64) <= 12.0)
            .then_some((other, target))
    }

    fn merge_endpoints(&mut self, dragged: Uuid, target: Uuid) -> bool {
        let Some((sub, first, other)) = self.endpoint(dragged) else {
            return false;
        };
        if other != target || self.ordered_vertices(sub).len() < 3 {
            return false;
        }
        let handle = {
            let vertices = self.state.drawing.vertex.peek();
            let source = &vertices[&dragged];
            if first {
                VertexOp::OutgoingHandle(*source.outgoing_handle.peek())
            } else {
                VertexOp::IncomingHandle(*source.incoming_handle.peek())
            }
        };
        self.apply(vec![
            DrawingOp::Vertex((target, vec![handle])),
            DrawingOp::Vertex((dragged, vec![VertexOp::Deleted(true)])),
            DrawingOp::SubPath((sub, vec![SubPathOp::Closed(true)])),
        ]);
        true
    }

    pub(super) fn insert_point(
        &mut self,
        sub: Uuid,
        from: Uuid,
        to: Uuid,
        target: path_geometry::Point,
    ) -> Option<Uuid> {
        // Resolve the current registers after the asynchronous coordinate lookup.
        let (a, b, order) = {
            let subs = self.state.drawing.sub_path.peek();
            let subpath = subs.get(&sub)?;
            if *subpath.deleted.peek() {
                return None;
            }
            let vertices = self.state.drawing.vertex.peek();
            let a = vertices.get(&from)?;
            let b = vertices.get(&to)?;
            if *a.deleted.peek()
                || *b.deleted.peek()
                || *a.parent.peek() != sub
                || *b.parent.peek() != sub
            {
                return None;
            }
            let resolve = |v: &crate::state::Vertex| {
                Some(ResolvedVertex {
                    pos: (*v.position.peek())?,
                    in_handle: *v.incoming_handle.peek(),
                    out_handle: *v.outgoing_handle.peek(),
                })
            };
            let ordered = self.ordered_vertices(sub);
            let index = ordered.iter().position(|v| v.1 == from)?;
            let closing =
                *subpath.closed.peek() && index + 1 == ordered.len() && ordered.first()?.1 == to;
            if !closing && ordered.get(index + 1)?.1 != to {
                return None;
            }
            let order = if closing {
                FractionalIndex::new_after(&a.sort_order.read())
            } else {
                FractionalIndex::new_between(&a.sort_order.read(), &b.sort_order.read())?
            };
            (resolve(a)?, resolve(b)?, order)
        };
        let t = path_geometry::nearest_parameter(path_geometry::controls(a, b), target);
        let split = path_geometry::split(a, b, t);
        if split.vertex.pos == a.pos || split.vertex.pos == b.pos {
            return None;
        }
        let id = self.id();
        self.apply(vec![
            DrawingOp::Vertex((from, vec![VertexOp::OutgoingHandle(split.outgoing)])),
            DrawingOp::Vertex((to, vec![VertexOp::IncomingHandle(split.incoming)])),
            DrawingOp::Vertex((
                id,
                vec![
                    VertexOp::Parent(sub),
                    VertexOp::SortOrder(order.as_bytes().to_vec()),
                    VertexOp::Position(Some(split.vertex.pos)),
                    VertexOp::IncomingHandle(split.vertex.in_handle),
                    VertexOp::OutgoingHandle(split.vertex.out_handle),
                ],
            )),
        ]);
        Some(id)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Extension {
    pub endpoint: Uuid,
    pub canvas_offset: (f64, f64),
}

async fn canvas_point(x: f64, y: f64) -> Option<[f64; 2]> {
    document::eval(&format!(
        r#"
        const svg = document.getElementById('drawing-canvas');
        const p = new DOMPoint({x}, {y}).matrixTransform(svg.getScreenCTM().inverse());
        return [p.x, p.y];
    "#
    ))
    .join::<[f64; 2]>()
    .await
    .ok()
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum DragKind {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    Vertex,
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    pub id: Uuid,
    pub pointer_id: i32,
    pub kind: DragKind,
    pub start: (f64, f64),
    pub position: XY,
    pub radius: XY,
    pub moved: bool,
}

impl Drag {
    fn update(&mut self, pointer: (f64, f64)) -> bool {
        self.moved |= (pointer.0 - self.start.0).hypot(pointer.1 - self.start.1) >= 2.0;
        self.moved
    }

    fn value_at(self, pointer: (f64, f64)) -> XY {
        let dx = (pointer.0 - self.start.0).round() as i64;
        let dy = (pointer.1 - self.start.1).round() as i64;
        match self.kind {
            DragKind::Move | DragKind::Vertex | DragKind::Incoming | DragKind::Outgoing => XY {
                x: self.position.x + dx,
                y: self.position.y + dy,
            },
            DragKind::Left => XY {
                x: (self.radius.x - dx).max(MIN_RADIUS),
                ..self.radius
            },
            DragKind::Right => XY {
                x: (self.radius.x + dx).max(MIN_RADIUS),
                ..self.radius
            },
            DragKind::Top => XY {
                y: (self.radius.y - dy).max(MIN_RADIUS),
                ..self.radius
            },
            DragKind::Bottom => XY {
                y: (self.radius.y + dy).max(MIN_RADIUS),
                ..self.radius
            },
        }
    }
}

#[component]
pub fn Drawing() -> Element {
    // All prototype state/signals are allocated under this drawing's scope.
    let model = use_hook(|| Rc::new(RefCell::new(Prototype::new())));
    use_context_provider(|| model.clone());
    let mut path_mode = use_signal(|| false);
    let mut selected = use_signal(|| None::<Uuid>);
    let mut selected_vertex = use_signal(|| None::<Uuid>);
    let mut drag = use_signal(|| None::<Drag>);
    let mut extending = use_signal(|| None::<Extension>);
    let mut preview_point = use_signal(|| None::<XY>);
    let mut close_target = use_signal(|| None::<(Uuid, XY)>);
    let ellipses = model.borrow().state.drawing.ellipse;
    let mut items = ellipses
        .iter()
        .map(|(id, item)| (id, item.peek().clone()))
        .collect::<Vec<_>>();
    items.sort_unstable_by_key(|(id, _)| *id);
    let drawing = model.borrow().state.drawing.clone();
    let path_members = model.borrow().state.path_sub_paths;
    let vertex_members = model.borrow().state.sub_path_vertices;
    let mut paths = drawing
        .path
        .iter()
        .map(|(id, item)| (id, item.peek().clone()))
        .collect::<Vec<_>>();
    paths.sort_unstable_by_key(|(id, _)| *id);
    let add_model = model.clone();
    let move_model = model.clone();
    let finish_model = model.clone();
    let extend_model = model.clone();
    let mut preview = String::new();
    if let Some(extension) = *extending.read()
        && let Some(pos) = *preview_point.read()
        && let Some((sub, first, _)) = model.borrow().endpoint(extension.endpoint)
    {
        let ordered = model.borrow().ordered_vertices(sub);
        let endpoint = if first {
            ordered.first()
        } else {
            ordered.last()
        }
        .unwrap()
        .2;
        let new = ResolvedVertex {
            pos,
            in_handle: None,
            out_handle: None,
        };
        let pair = if first {
            [new, endpoint]
        } else {
            [endpoint, new]
        };
        super::sub_path::render_subpath(&mut preview, pair.into_iter(), false);
    }

    rsx! {
        document::Stylesheet { href: asset!("/assets/styling/drawing.css") }
        main { class: "drawing-app", tabindex: "0",
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    extending.set(None);
                    preview_point.set(None);
                    close_target.set(None);
                }
            },
            header { class: "drawing-toolbar",
                div {
                    h1 { "Drawing playground" }
                    p { "Double-click canvas to add. Drag points to move. Double-click a selected path to insert a point." }
                    p { "Add Path: click an endpoint, then click to extend; Escape finishes. Click the other end to close, or drag one end onto the other." }
                }
                div { class: "drawing-tools",
                    button { class: if !path_mode() { "active" } else { "" }, aria_pressed: (!path_mode()).to_string(), onclick: move |_| { path_mode.set(false); extending.set(None); }, "Add ellipse" }
                    button { class: if path_mode() { "active" } else { "" }, aria_pressed: path_mode().to_string(), onclick: move |_| { path_mode.set(true); extending.set(None); }, "Add path" }
                }
                span { class: "drawing-badge", "Local prototype · not saved" }
            }
            svg {
                id: "drawing-canvas",
                class: "drawing-canvas",
                width: "100%",
                height: "100%",
                role: "img",
                "aria-label": "Drawing canvas",
                // Native capture must happen synchronously in the WebView, before
                // an asynchronous Rust event round trip can miss pointer release.
                onmounted: move |_| async move {
                    let _ = document::eval(r#"
                        const svg = document.getElementById('drawing-canvas');
                        if (svg && !svg.dataset.captureReady) {
                            svg.dataset.captureReady = 'true';
                            svg.addEventListener('pointerdown', e => {
                                if (e.isPrimary && e.button === 0) {
                                    svg.closest('main').focus({preventScroll: true});
                                    e.target.setPointerCapture(e.pointerId);
                                }
                            }, true);
                        }
                    "#).await;
                },
                onpointermove: move |event| {
                    let current = *drag.peek();
                    let p = event.client_coordinates();
                    if let Some(mut current) = current && current.pointer_id == event.pointer_id() {
                        if current.update((p.x, p.y)) {
                            extending.set(None);
                            apply_drag(&move_model, current, (p.x, p.y));
                            let target = if current.kind == DragKind::Vertex {
                                move_model.borrow().snap_target(current.id, current.value_at((p.x, p.y)))
                            } else { None };
                            close_target.set(target);
                            drag.set(Some(current));
                        }
                    } else if let Some(extension) = *extending.peek() {
                        preview_point.set(Some(XY {
                            x: (p.x - extension.canvas_offset.0).round() as i64,
                            y: (p.y - extension.canvas_offset.1).round() as i64,
                        }));
                    }
                },
                onpointerup: move |event| {
                    let current = *drag.peek();
                    if let Some(mut current) = current && current.pointer_id == event.pointer_id() {
                        let p = event.client_coordinates();
                        if current.update((p.x, p.y)) {
                            extending.set(None);
                            apply_drag(&finish_model, current, (p.x, p.y));
                            let target = if current.kind == DragKind::Vertex {
                                finish_model.borrow().snap_target(current.id, current.value_at((p.x, p.y)))
                            } else { None };
                            if let Some((id, _)) = target
                                && finish_model.borrow_mut().merge_endpoints(current.id, id)
                            { selected_vertex.set(Some(id)); }
                        } else if current.kind == DragKind::Vertex && *path_mode.peek() {
                            let previous = *extending.peek();
                            let closed = previous.is_some_and(|e| finish_model.borrow_mut().close(e.endpoint, current.id));
                            if closed { extending.set(None); }
                            else if finish_model.borrow().endpoint(current.id).is_some() {
                                let pending = Extension { endpoint: current.id, canvas_offset: (p.x - current.position.x as f64, p.y - current.position.y as f64) };
                                extending.set(Some(pending));
                                preview_point.set(Some(current.position));
                                spawn(async move {
                                    if let Some([x, y]) = canvas_point(p.x, p.y).await
                                        && *extending.peek() == Some(pending)
                                    {
                                        extending.set(Some(Extension { canvas_offset: (p.x - x, p.y - y), ..pending }));
                                    }
                                });
                            } else { extending.set(None); }
                        }
                        close_target.set(None);
                        drag.set(None);
                    }
                },
                onpointercancel: move |_| { drag.set(None); close_target.set(None); },
                onlostpointercapture: move |_| { drag.set(None); close_target.set(None); },
                rect {
                    width: "100%", height: "100%", fill: "#f8fafc",
                    onpointerdown: move |event| {
                        if event.is_primary() && event.trigger_button() == Some(MouseButton::Primary) && extending.peek().is_none() {
                            selected.set(None);
                            selected_vertex.set(None);
                        }
                    },
                    onclick: move |event| {
                        if event.trigger_button() != Some(MouseButton::Primary) { return; }
                        let current = *extending.peek();
                        if let Some(extension) = current {
                            let p = event.element_coordinates();
                            let position = XY { x: p.x.round() as i64, y: p.y.round() as i64 };
                            if let Some(id) = extend_model.borrow_mut().extend(extension.endpoint, position) {
                                let client = event.client_coordinates();
                                extending.set(Some(Extension { endpoint: id, canvas_offset: (client.x - p.x, client.y - p.y) }));
                                selected_vertex.set(Some(id));
                                preview_point.set(Some(position));
                            }
                        }
                    },
                    ondoubleclick: move |event| {
                        if event.trigger_button() != Some(MouseButton::Primary) || extending.peek().is_some() { return; }
                        let p = event.element_coordinates();
                        let position = XY { x: p.x.round() as i64, y: p.y.round() as i64 };
                        let id = if *path_mode.peek() { add_model.borrow_mut().add_path(position) } else { add_model.borrow_mut().add_circle(position) };
                        selected.set(Some(id));
                        selected_vertex.set(None);
                    },
                }
                for (id, state) in items {
                    EditableEllipse {
                        key: "{id:?}", id, state, selected, drag, extending,
                    }
                }
                for (id, state) in paths {
                    EditablePath {
                        key: "{id:?}", id, state, drawing: drawing.clone(),
                        members: *path_members.peek().get(&id).expect("path membership initialized"),
                        vertex_members, selected, selected_vertex, drag, extending,
                    }
                }
                path { d: preview, fill: "none", stroke: "#0f766e", stroke_width: 2, stroke_dasharray: "5 4", pointer_events: "none" }
                if let Some((_, position)) = *close_target.read() {
                    circle { cx: position.x, cy: position.y, r: 13, fill: "none", stroke: "#f59e0b", stroke_width: 3, pointer_events: "none" }
                    text { x: position.x + 16, y: position.y - 16, fill: "#92400e", font_size: 14, pointer_events: "none", "Release to close" }
                }
            }
        }
    }
}

fn apply_drag(model: &Rc<RefCell<Prototype>>, drag: Drag, pointer: (f64, f64)) {
    let value = drag.value_at(pointer);
    let vertex_op = match drag.kind {
        DragKind::Vertex => Some(VertexOp::Position(Some(value))),
        DragKind::Incoming => Some(VertexOp::IncomingHandle(Some(value))),
        DragKind::Outgoing => Some(VertexOp::OutgoingHandle(Some(value))),
        _ => None,
    };
    if let Some(op) = vertex_op {
        model
            .borrow_mut()
            .apply(vec![DrawingOp::Vertex((drag.id, vec![op]))]);
        return;
    }
    let op = match drag.kind {
        DragKind::Move => EllipseOp::Position(Some(value)),
        _ => EllipseOp::Radius(Some(value)),
    };
    model.borrow_mut().edit(drag.id, vec![op]);
}

#[component]
fn EditableEllipse(
    id: Uuid,
    state: EllipseState,
    mut selected: Signal<Option<Uuid>>,
    mut drag: Signal<Option<Drag>>,
    mut extending: Signal<Option<Extension>>,
) -> Element {
    if *state.deleted.read() {
        return rsx! {};
    }
    let Some(position) = *state.position.read() else {
        return rsx! {};
    };
    let Some(radius) = *state.radius.read() else {
        return rsx! {};
    };
    let active = *selected.read() == Some(id);
    let mut start = move |event: PointerEvent, kind: DragKind| {
        if !event.is_primary() || event.trigger_button() != Some(MouseButton::Primary) {
            return;
        }
        event.stop_propagation();
        let p = event.client_coordinates();
        selected.set(Some(id));
        extending.set(None);
        drag.set(Some(Drag {
            id,
            pointer_id: event.pointer_id(),
            kind,
            start: (p.x, p.y),
            position,
            radius,
            moved: false,
        }));
    };
    let handles = [
        (
            DragKind::Left,
            position.x - radius.x,
            position.y,
            "ew-resize",
        ),
        (
            DragKind::Right,
            position.x + radius.x,
            position.y,
            "ew-resize",
        ),
        (
            DragKind::Top,
            position.x,
            position.y - radius.y,
            "ns-resize",
        ),
        (
            DragKind::Bottom,
            position.x,
            position.y + radius.y,
            "ns-resize",
        ),
    ];
    rsx! {
        g {
            ondoubleclick: move |event| event.stop_propagation(),
            g {
                style: "cursor: move",
                onpointerdown: move |event| start(event, DragKind::Move),
                Ellipse { state }
            }
            if active {
                rect {
                    x: position.x - radius.x, y: position.y - radius.y,
                    width: radius.x * 2, height: radius.y * 2,
                    fill: "none", stroke: "#60a5fa", stroke_dasharray: "4 4", pointer_events: "none",
                }
                for (kind, x, y, cursor) in handles {
                    circle {
                        cx: x, cy: y, r: 9, fill: "white", stroke: "#2563eb", stroke_width: 2,
                        style: "cursor: {cursor}",
                        onpointerdown: move |event| start(event, kind),
                    }
                }
                circle {
                    cx: position.x, cy: position.y, r: 10,
                    fill: "#2563eb", stroke: "white", stroke_width: 2,
                    style: "cursor: move",
                    onpointerdown: move |event| start(event, DragKind::Move),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(kind: DragKind) -> Drag {
        Drag {
            id: [1; 16],
            pointer_id: 1,
            kind,
            start: (100.0, 200.0),
            position: XY { x: 80, y: 90 },
            radius: XY { x: 30, y: 40 },
            moved: false,
        }
    }

    #[test]
    fn moving_preserves_grab_offset_and_uses_original_position() {
        let d = drag(DragKind::Move);
        assert_eq!(d.value_at((115.0, 192.0)), XY { x: 95, y: 82 });
        assert_eq!(d.value_at((120.0, 210.0)), XY { x: 100, y: 100 });
    }

    #[test]
    fn clicks_do_not_activate_handles_but_drags_can_return_to_the_start() {
        let mut d = drag(DragKind::Outgoing);
        assert!(!d.update((100.0, 200.0)));
        assert!(!d.update((101.0, 200.0)));
        assert!(d.update((110.0, 200.0)));
        assert!(d.update((100.0, 200.0)));
        assert_eq!(d.value_at((100.0, 200.0)), d.position);
    }

    #[test]
    fn path_insertions_update_membership_order_and_both_adjacent_handles() {
        let mut dom = VirtualDom::new(|| rsx! {});
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::ROOT, || {
            let mut model = Prototype::new();
            let path = model.add_path(XY { x: 200, y: 200 });
            let sub = *model.state.path_sub_paths.peek()[&path]
                .peek()
                .iter()
                .next()
                .unwrap();
            let ordered_ids = |model: &Prototype| {
                let vertices = model.state.drawing.vertex.peek();
                let indexes = model.state.sub_path_vertices.peek();
                let mut entries = indexes[&sub]
                    .peek()
                    .iter()
                    .map(|id| (vertices[id].sort_order.read().clone(), *id))
                    .collect::<Vec<_>>();
                entries.sort();
                entries.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
            };
            let original = ordered_ids(&model);
            let (a, b) = (original[0], original[1]);
            let mid = model
                .insert_point(sub, a, b, path_geometry::Point(200.0, 205.0))
                .unwrap();
            assert_eq!(ordered_ids(&model), vec![a, mid, b]);
            assert_eq!(
                *model.state.drawing.vertex.peek()[&mid].position.peek(),
                Some(XY { x: 200, y: 200 })
            );
            // An obsolete target must not split a segment that no longer exists.
            assert!(
                model
                    .insert_point(sub, a, b, path_geometry::Point(180.0, 200.0))
                    .is_none()
            );

            model.apply(vec![DrawingOp::Vertex((
                mid,
                vec![VertexOp::OutgoingHandle(Some(XY { x: 30, y: -60 }))],
            ))]);
            let before_a = ResolvedVertex {
                pos: XY { x: 200, y: 200 },
                in_handle: None,
                out_handle: Some(XY { x: 30, y: -60 }),
            };
            let before_b = ResolvedVertex {
                pos: XY { x: 260, y: 200 },
                in_handle: None,
                out_handle: None,
            };
            let target = path_geometry::evaluate(path_geometry::controls(before_a, before_b), 0.5);
            let inserted = model.insert_point(sub, mid, b, target).unwrap();
            assert_eq!(ordered_ids(&model), vec![a, mid, inserted, b]);
            let vertices = model.state.drawing.vertex.peek();
            let resolve = |id: Uuid| ResolvedVertex {
                pos: vertices[&id].position.peek().unwrap(),
                in_handle: *vertices[&id].incoming_handle.peek(),
                out_handle: *vertices[&id].outgoing_handle.peek(),
            };
            let (left, middle, right) = (resolve(mid), resolve(inserted), resolve(b));
            assert!(left.out_handle.is_some() && middle.in_handle.is_some());
            assert!(middle.out_handle.is_some() && right.in_handle.is_some());
            assert_eq!(resolve(a).out_handle, None);
            assert_eq!(left.in_handle, None);
            for i in 0..=100 {
                let t = i as f64 / 100.0;
                let before =
                    path_geometry::evaluate(path_geometry::controls(before_a, before_b), t);
                let after = if t <= 0.5 {
                    path_geometry::evaluate(path_geometry::controls(left, middle), t * 2.0)
                } else {
                    path_geometry::evaluate(path_geometry::controls(middle, right), (t - 0.5) * 2.0)
                };
                assert!((before.0 - after.0).hypot(before.1 - after.1) < 1.5);
            }
        });
    }

    fn with_path(f: impl FnOnce(&mut Prototype, Uuid, Uuid, Uuid)) {
        let mut dom = VirtualDom::new(|| rsx! {});
        dom.rebuild_in_place();
        dom.in_scope(ScopeId::ROOT, || {
            let mut model = Prototype::new();
            let path = model.add_path(XY { x: 200, y: 200 });
            let sub = *model.state.path_sub_paths.peek()[&path]
                .peek()
                .iter()
                .next()
                .unwrap();
            let vertices = model.ordered_vertices(sub);
            f(&mut model, sub, vertices[0].1, vertices[1].1);
        });
    }

    #[test]
    fn extend_both_ends_preserves_existing_geometry_and_rejects_interior_or_closed_vertices() {
        with_path(|model, sub, a, b| {
            model.apply(vec![DrawingOp::Vertex((
                a,
                vec![VertexOp::OutgoingHandle(Some(XY { x: 30, y: -20 }))],
            ))]);
            let original = model.ordered_vertices(sub);
            let first = model.extend(a, XY { x: 80, y: 100 }).unwrap();
            let last = model.extend(b, XY { x: 320, y: 100 }).unwrap();
            let ordered = model.ordered_vertices(sub);
            assert_eq!(
                ordered.iter().map(|v| v.1).collect::<Vec<_>>(),
                vec![first, a, b, last]
            );
            assert!(ordered.windows(2).all(|p| p[0].0 < p[1].0));
            assert!(ordered[1].2 == original[0].2 && ordered[2].2 == original[1].2);
            assert!(model.extend(a, XY { x: 0, y: 0 }).is_none());
            assert!(!model.close(first, b));
            assert!(model.close(first, last));
            assert!(model.extend(first, XY { x: 0, y: 0 }).is_none());
            assert_eq!(model.ordered_vertices(sub).len(), 4);
        });
    }

    #[test]
    fn dragging_either_end_onto_the_other_preserves_used_handles_and_removes_duplicate_vertex() {
        for drag_first in [false, true] {
            with_path(|model, sub, a, b| {
                let last = model.extend(b, XY { x: 260, y: 300 }).unwrap();
                let outgoing = XY { x: 35, y: -25 };
                let incoming = XY { x: -20, y: 45 };
                model.apply(vec![
                    DrawingOp::Vertex((a, vec![VertexOp::OutgoingHandle(Some(outgoing))])),
                    DrawingOp::Vertex((last, vec![VertexOp::IncomingHandle(Some(incoming))])),
                ]);
                let (dragged, target) = if drag_first { (a, last) } else { (last, a) };
                let position = model.state.drawing.vertex.peek()[&target]
                    .position
                    .peek()
                    .unwrap();
                assert!(
                    model
                        .snap_target(
                            dragged,
                            XY {
                                x: position.x + 13,
                                ..position
                            }
                        )
                        .is_none()
                );
                assert_eq!(
                    model.snap_target(
                        dragged,
                        XY {
                            x: position.x + 10,
                            ..position
                        }
                    ),
                    Some((target, position))
                );
                assert!(model.snap_target(b, position).is_none());
                assert!(model.merge_endpoints(dragged, target));
                assert!(*model.state.drawing.sub_path.peek()[&sub].closed.peek());
                let vertices = model.ordered_vertices(sub);
                assert_eq!(vertices.len(), 2);
                assert!(!vertices.iter().any(|v| v.1 == dragged));
                let merged = vertices.iter().find(|v| v.1 == target).unwrap().2;
                assert_eq!(merged.pos, position);
                assert_eq!(merged.in_handle, Some(incoming));
                assert_eq!(merged.out_handle, Some(outgoing));
                assert!(*model.state.drawing.vertex.peek()[&dragged].deleted.peek());
                assert!(model.endpoint(target).is_none());
                assert!(!model.merge_endpoints(dragged, target));
            });
        }
    }

    #[test]
    fn two_point_path_cannot_be_collapsed_to_one_by_dragging() {
        with_path(|model, _, a, b| {
            assert!(model.snap_target(a, XY { x: 260, y: 200 }).is_none());
            assert!(!model.merge_endpoints(a, b));
        });
    }

    #[test]
    fn closing_segment_can_be_split_repeatedly_and_keeps_its_curve() {
        with_path(|model, sub, a, b| {
            let end = model.extend(b, XY { x: 260, y: 320 }).unwrap();
            model.apply(vec![
                DrawingOp::Vertex((
                    end,
                    vec![VertexOp::OutgoingHandle(Some(XY { x: -20, y: 40 }))],
                )),
                DrawingOp::Vertex((
                    a,
                    vec![VertexOp::IncomingHandle(Some(XY { x: -50, y: 20 }))],
                )),
            ]);
            assert!(model.close(end, a));
            let before = model.ordered_vertices(sub);
            let curve = path_geometry::controls(before[2].2, before[0].2);
            let inserted = model
                .insert_point(sub, end, a, path_geometry::evaluate(curve, 0.5))
                .unwrap();
            let after = model.ordered_vertices(sub);
            assert_eq!(
                after.iter().map(|v| v.1).collect::<Vec<_>>(),
                vec![a, b, end, inserted]
            );
            for i in 0..=100 {
                let t = i as f64 / 100.0;
                let original = path_geometry::evaluate(curve, t);
                let split = if t <= 0.5 {
                    path_geometry::evaluate(
                        path_geometry::controls(after[2].2, after[3].2),
                        t * 2.0,
                    )
                } else {
                    path_geometry::evaluate(
                        path_geometry::controls(after[3].2, after[0].2),
                        (t - 0.5) * 2.0,
                    )
                };
                assert!((original.0 - split.0).hypot(original.1 - split.1) < 1.5);
            }
            assert!(
                model
                    .insert_point(sub, end, a, path_geometry::Point(190.0, 280.0))
                    .is_none()
            );
            let next_curve = path_geometry::controls(after[3].2, after[0].2);
            assert!(
                model
                    .insert_point(sub, inserted, a, path_geometry::evaluate(next_curve, 0.5))
                    .is_some()
            );
            let mut d = String::new();
            super::super::sub_path::render_subpath(
                &mut d,
                model.ordered_vertices(sub).into_iter().map(|v| v.2),
                true,
            );
            assert!(d.ends_with(" Z"));
            assert_eq!(d.matches(" C ").count(), 3);
        });
    }

    #[test]
    fn edges_resize_only_their_axis_and_stop_at_minimum() {
        assert_eq!(
            drag(DragKind::Left).value_at((90.0, 250.0)),
            XY { x: 40, y: 40 }
        );
        assert_eq!(
            drag(DragKind::Right).value_at((110.0, 250.0)),
            XY { x: 40, y: 40 }
        );
        assert_eq!(
            drag(DragKind::Top).value_at((150.0, 190.0)),
            XY { x: 30, y: 50 }
        );
        assert_eq!(
            drag(DragKind::Bottom).value_at((150.0, 210.0)),
            XY { x: 30, y: 50 }
        );
        assert_eq!(
            drag(DragKind::Left).value_at((200.0, 200.0)),
            XY {
                x: MIN_RADIUS,
                y: 40
            }
        );
    }
}
