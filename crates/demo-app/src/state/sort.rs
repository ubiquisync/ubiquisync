use loro_fractional_index::FractionalIndex;
use ubiquisync_core::hlc::Timestamp;

use crate::state::Init;
use crate::state::lww::{Apply, Lww};

#[derive(Default, Clone, PartialEq)]
pub struct SortOrder {
    state: Lww<FractionalIndex>,
}

impl SortOrder {
    pub fn read(&self) -> impl std::ops::Deref<Target = FractionalIndex> + '_ {
        self.state.read()
    }
}

impl Init for SortOrder {
    fn init() -> Self {
        Self::default()
    }
}

impl Apply for SortOrder {
    type Op = Vec<u8>;

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        self.state.apply(ts, FractionalIndex::from_bytes(op));
    }
}
