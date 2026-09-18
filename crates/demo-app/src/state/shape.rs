use std::collections::{HashMap, HashSet};

use borsh::{BorshDeserialize, BorshSerialize};
use dioxus::{
    signals::{ReadableExt, Signal, WritableHashSetExt},
    stores::Store,
};
use ubiquisync_core::{hlc, uuid::Uuid};

use crate::{
    def_state,
    state::{
        Init,
        lww::{Apply, Lww},
        sort::SortOrder,
    },
};

pub struct DrawingState {
    pub drawing: Drawing,
    pub path_sub_paths: Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
    pub sub_path_vertices: Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
}

impl Init for DrawingState {
    fn init() -> Self {
        Self {
            drawing: Drawing::init(),
            path_sub_paths: Init::init(),
            sub_path_vertices: Init::init(),
        }
    }
}

def_state!(Drawing {
    ellipse: Store<HashMap<Uuid, Ellipse>>,
    text: Store<HashMap<Uuid, Text>>,
    path: Store<HashMap<Uuid, Path>>,
    sub_path: Store<HashMap<Uuid, SubPath>>,
    vertex: Store<HashMap<Uuid, Vertex>>,

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
    fill_stroke: FillStroke,
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
            DrawingOp::Path((id, _)) => {
                ensure_membership(&mut self.path_sub_paths, id);
                self.drawing.apply(ts, op);
            }
            DrawingOp::SubPath((id, _)) => {
                ensure_membership(&mut self.sub_path_vertices, id);
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

fn get_parent<F, S: 'static>(state: &Store<HashMap<Uuid, S>>, id: &Uuid, accessor: F) -> Uuid
where
    F: Fn(&S) -> &Lww<Uuid>,
{
    state
        .peek()
        .get(id)
        .map(|s| accessor(s).peek().clone())
        .unwrap_or_default()
}

// Preserve the signal identity, including when children arrive before their parent.
fn ensure_membership(
    index: &mut Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
    id: Uuid,
) -> Signal<HashSet<Uuid>> {
    let existing = index.peek().get(&id).copied();
    if let Some(members) = existing {
        return members;
    }
    let members = Signal::default();
    index.insert(id, members);
    members
}

fn update_parent_index(
    index: &mut Store<HashMap<Uuid, Signal<HashSet<Uuid>>>>,
    cur_parent: Uuid,
    new_parent: Uuid,
    id: Uuid,
) {
    if cur_parent == new_parent {
        return;
    }

    if cur_parent != [0; 16]
        && let Some(mut cur) = index.peek().get(&cur_parent).copied()
    {
        cur.remove(&id);
    }

    if new_parent != [0; 16] {
        ensure_membership(index, new_parent).insert(id);
    }
}
