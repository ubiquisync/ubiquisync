use ubiquisync_core::uuid::Uuid;

pub enum Op {
    Drawing { doc: Uuid, op: DrawingOp },
}

pub enum DrawingOp {}
