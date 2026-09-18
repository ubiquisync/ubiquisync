use dioxus::prelude::*;

use crate::state::Ellipse as EllipseState;

#[component]
pub fn Ellipse(state: EllipseState) -> Element {
    rsx! {
        ellipse {

        }
    }
}
