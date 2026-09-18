use crate::state::XY;
use std::fmt::Write;

#[derive(Clone, Copy)]
pub struct ResolvedVertex {
    pub pos: XY,
    pub in_handle: Option<XY>,
    pub out_handle: Option<XY>,
}

// widen output to avoid overflow
fn add_xy(pos: &XY, offset: &XY) -> (i128, i128) {
    (
        pos.x as i128 + offset.x as i128,
        pos.y as i128 + offset.y as i128,
    )
}

fn render_subpath(
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
