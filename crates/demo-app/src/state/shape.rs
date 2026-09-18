use std::collections::{HashMap, HashSet};

use borsh::{BorshDeserialize, BorshSerialize};
use dioxus::signals::{Signal, WritableHashSetExt};
use ubiquisync_core::{hlc, uuid::Uuid};

use crate::{
    def_state,
    state::{
        lww::{Apply, Lww, StateMap},
        sort::SortOrder,
    },
};

pub struct DrawingState {
    pub drawing: Drawing,
    pub path_sub_paths: HashMap<Uuid, Signal<HashSet<Uuid>>>,
    pub sub_path_vertices: HashMap<Uuid, Signal<HashSet<Uuid>>>,
}

def_state!(Drawing {
    ellipse: Signal<HashMap<Uuid, Ellipse>>,
    text: Signal<HashMap<Uuid, Text>>,
    path: Signal<HashMap<Uuid, Path>>,

    // these don't need signals because we can detect membership with the indexes
    sub_path: StateMap<SubPath>,
    vertex: StateMap<Vertex>,

    deleted: Lww<bool>
});

def_state!(Path {
    fill_stroke: FillStroke,
    deleted: Lww<bool>
});

def_state!(SubPath {
    parent: Lww<Uuid>,
    sort_order: SortOrder,
    closed: Lww<bool>,
    deleted: Lww<bool>
});

def_state!(Vertex {
    parent: Lww<Uuid>,
    incoming_handle: Lww<Option<XY>>,
    position: Lww<Option<XY>>,
    outgoing_handle: Lww<Option<XY>>,
    sort_order: SortOrder,
    deleted: Lww<bool>
});

def_state!(Ellipse {
    position: Lww<Option<XY>>,
    radius: Lww<Option<XY>>,
    fill_stroke: FillStroke,
    deleted: Lww<bool>
});

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
    deleted: Lww<bool>
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct XY {
    pub x: i64,
    pub y: i64,
}

impl Apply for DrawingState {
    type Op = DrawingOp;

    fn apply(&mut self, ts: hlc::Timestamp, op: Self::Op) {
        match op {
            DrawingOp::SubPath((id, _)) => {
                let cur_parent = get_parent(&self.drawing.sub_path, &id, |s| &s.parent);
                self.drawing.apply(ts, op);
                let new_parent = get_parent(&self.drawing.sub_path, &id, |s| &s.parent);
                update_parent_index(&mut self.path_sub_paths, cur_parent, new_parent, id);
            }
            DrawingOp::Vertex((id, _)) => {
                let cur_parent = get_parent(&self.drawing.vertex, &id, |s| &s.parent);
                self.drawing.apply(ts, op);
                let new_parent = get_parent(&self.drawing.vertex, &id, |s| &s.parent);
                update_parent_index(&mut self.sub_path_vertices, cur_parent, new_parent, id);
            }
            op => self.drawing.apply(ts, op),
        }
    }
}

fn get_parent<F, S>(state: &StateMap<S>, id: &Uuid, accessor: F) -> Uuid
where
    F: Fn(&S) -> &Lww<Uuid>,
{
    state
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|s| accessor(s).peek())
        .unwrap_or_default()
}

fn update_parent_index(
    index: &mut HashMap<Uuid, Signal<HashSet<Uuid>>>,
    cur_parent: Uuid,
    new_parent: Uuid,
    id: Uuid,
) {
    if cur_parent == new_parent {
        return;
    }

    if cur_parent != [0; 16]
        && let Some(cur) = index.get_mut(&cur_parent)
    {
        cur.remove(&id);
    }

    if new_parent != [0; 16] {
        index.entry(new_parent).or_default().insert(id);
    }
}
