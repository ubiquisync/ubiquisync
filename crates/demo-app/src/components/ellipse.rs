use dioxus::prelude::*;

use crate::state::Ellipse as EllipseState;

#[component]
pub fn Ellipse(state: EllipseState) -> Element {
    if let Some(position) = *state.position.read()
        && let Some(radius) = *state.radius.read()
        && !*state.deleted.read()
    {
        rsx! {
            ellipse {
                cx: position.x,
                cy: position.y,
                rx: radius.x,
                ry: radius.y,
            }
        }
    } else {
        rsx! {} // is this okay for a empty?
    }
}
