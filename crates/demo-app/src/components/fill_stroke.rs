use dioxus::prelude::*;

use crate::state::FillStroke;

pub fn fill_stroke_attrs(style: &FillStroke) -> Vec<Attribute> {
    [
        ("stroke", style.stroke.read().clone()),
        ("stroke-width", style.stroke_width.read().clone()),
        ("stroke-opacity", style.stroke_opacity.read().clone()),
        ("fill", style.fill.read().clone()),
        ("fill-opacity", style.fill_opacity.read().clone()),
    ]
    .into_iter()
    .map(|(name, value)| Attribute::new(name, value, None, false))
    .collect()
}
