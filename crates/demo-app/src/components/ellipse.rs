use dioxus::prelude::*;

use crate::state::Ellipse as EllipseState;

use super::fill_stroke::fill_stroke_attrs;

#[component]
pub fn Ellipse(state: EllipseState) -> Element {
    if let Some(position) = *state.position.read()
        && let Some(radius) = *state.radius.read()
        && !*state.deleted.read()
    {
        let attrs = fill_stroke_attrs(&state.fill_stroke);
        rsx! {
            ellipse {
                cx: position.x,
                cy: position.y,
                rx: radius.x,
                ry: radius.y,
                ..attrs
            }
        }
    } else {
        // nothing to render
        rsx! {}
    }
}
