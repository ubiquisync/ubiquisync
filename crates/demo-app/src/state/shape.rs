use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use ubiquisync_core::uuid::Uuid;

use crate::{def_state, state::lww::Lww};

pub struct Drawing {
    paths: HashMap<Uuid, Path>,
    ellipses: HashMap<Uuid, Ellipse>,
    text: HashMap<Uuid, Text>,
}

pub struct Path {
    sub_paths: HashMap<Uuid, SubPath>,
}

pub struct SubPath {
    vertices: HashMap<Uuid, Vertex>,
    sort_order: Lww<Vec<u8>>,
    closed: Lww<bool>,
}

def_state!(Vertex {
    incoming_handle: Lww<Option<XY>>,
    position: Lww<Option<XY>>,
    outgoing_handle: Lww<Option<XY>>,
    sort_order: Lww<Vec<u8>>,
    fill_stroke: FillStroke,
});

def_state!(Ellipse {
    position: Lww<Option<XY>>,
    radius: Lww<Option<XY>>,
    fill_stroke: FillStroke,
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct XY {
    pub x: i64,
    pub y: i64,
}

def_state!(FillStroke {
    stroke: Lww<String>,
    stroke_width: Lww<String>,
    stroke_opacity: Lww<String>,
    fill: Lww<String>,
    fill_opacity: Lww<String>,
});

def_state!(Text {
    text: Lww<String>,
    position: Lww<Option<XY>>,
    fill: Lww<String>,
    font: Lww<String>,
    size: Lww<u64>,
});
