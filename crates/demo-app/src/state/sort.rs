use loro_fractional_index::FractionalIndex;
use ubiquisync_core::hlc::Timestamp;

use crate::state::lww::{Apply, Lww};

#[derive(Default, Clone, PartialEq)]
pub struct SortOrder {
    state: Lww<FractionalIndex>,
}

impl Apply for SortOrder {
    type Op = Vec<u8>;

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        self.state.apply(ts, FractionalIndex::from_bytes(op));
    }
}
