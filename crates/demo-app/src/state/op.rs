use ubiquisync_core::uuid::Uuid;

use crate::state::DrawingOp;

pub enum Op {
    Drawing { doc: Uuid, op: Vec<DrawingOp> },
}
