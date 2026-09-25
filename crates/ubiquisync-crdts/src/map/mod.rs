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

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key).and_then(|r| r.get().as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.map
            .iter()
            .filter_map(|(k, r)| r.get().as_ref().map(|v| (k, v)))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|e| e.0)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|e| e.1)
    }
}
