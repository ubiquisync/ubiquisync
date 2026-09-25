use std::cmp::Ordering;

use ubiquisync_core::hlc::Timestamp;

#[derive(Default)]
pub struct LwwRegister<V> {
    timestamp: Timestamp,
    value: V,
}

impl<V: Ord> LwwRegister<V> {
    pub fn apply(&mut self, timestamp: Timestamp, value: V) {
        match self.timestamp.cmp(&timestamp) {
            Ordering::Less => {
                self.timestamp = timestamp;
                self.value = value;
            }
            Ordering::Equal => {
                if self.value.lt(&value) {
                    self.value = value;
                }
            }
            Ordering::Greater => {}
        }
    }
}
