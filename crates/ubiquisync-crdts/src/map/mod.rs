use std::collections::HashMap;

use ubiquisync_core::hlc::Timestamp;

use crate::lww::LwwRegister;

#[derive(Default)]
pub struct Map<K, V> {
    map: HashMap<K, LwwRegister<Option<V>>>,
}

pub enum Op<K, V> {
    Insert(K, V),
    Remove(K),
}

impl<K: std::hash::Hash + Eq, V: Ord> Map<K, V> {
    pub fn apply(&mut self, timestamp: Timestamp, op: Op<K, V>) {
        let (k, v) = match op {
            Op::Insert(k, v) => (k, Some(v)),
            Op::Remove(k) => (k, None),
        };

        self.map.entry(k).or_default().apply(timestamp, v);
    }
}
