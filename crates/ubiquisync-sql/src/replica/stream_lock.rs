use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use futures::lock::OwnedMutexGuard;

#[derive(Default, Clone)]
pub(crate) struct KeyedLock<K>(Arc<std::sync::Mutex<HashMap<K, Weak<futures::lock::Mutex<()>>>>>);

pub(crate) struct KeyedLockGuard<K: std::hash::Hash + Eq> {
    key: K,
    locks: KeyedLock<K>,
    guard: Option<OwnedMutexGuard<()>>,
}

impl<K: std::hash::Hash + Eq + Clone> KeyedLock<K> {
    pub(crate) async fn lock(&self, key: &K) -> KeyedLockGuard<K> {
        let mtx = {
            let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(m) = map.get(key).and_then(Weak::upgrade) {
                m
            } else {
                let m = Arc::new(Default::default());
                map.insert(key.clone(), Arc::downgrade(&m));
                m
            }
        };
        let guard = mtx.lock_owned().await;
        KeyedLockGuard {
            key: key.clone(),
            locks: self.clone(),
            guard: Some(guard),
        }
    }
}

impl<K: std::hash::Hash + Eq> Drop for KeyedLockGuard<K> {
    fn drop(&mut self) {
        self.guard.take();
        let mut map = self.locks.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(weak) = map.get(&self.key) {
            if weak.strong_count() == 0 {
                map.remove(&self.key);
            }
        }
    }
}
