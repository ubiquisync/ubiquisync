use dioxus::prelude::*;

use crate::state::Text as TextState;

use super::fill_stroke::fill_stroke_attrs;

#[component]
pub fn Text(state: TextState) -> Element {
    if !*state.deleted.read()
        && let Some(position) = *state.position.read()
    {
        let attrs = fill_stroke_attrs(&state.fill_stroke);
        let content = state.text.read().clone();
        let font = state.font.read().clone();
        let size = *state.size.read();

        rsx! {
            text {
                x: position.x,
                y: position.y,
                font_family: font,
                font_size: size,
                ..attrs,
                "{content}"
            }
        }
    } else {
        rsx! {}
    }
}
