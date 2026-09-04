use std::{
    collections::HashMap,
    sync::{Arc, MutexGuard, Weak},
};

use futures::lock::OwnedMutexGuard;

#[derive(Clone)]
pub(crate) struct KeyedLock<K>(Arc<std::sync::Mutex<HashMap<K, Weak<futures::lock::Mutex<()>>>>>);

pub(crate) struct KeyedLockGuard<K: std::hash::Hash + Eq + Clone> {
    key: K,
    locks: KeyedLock<K>,
    // the option here exists murely to let us take the guard and release the lock
    // before we try dispose of the lock, otherwise the Arc strong count will never go to 0
    guard: Option<OwnedMutexGuard<()>>,
}

impl<K: std::hash::Hash + Eq + Clone> KeyedLock<K> {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Default::default()))
    }

    pub(crate) async fn lock(&self, key: &K) -> KeyedLockGuard<K> {
        let mtx = {
            let mut map = self.lock_map();
            map.get(key).and_then(Weak::upgrade).unwrap_or_else(|| {
                let m = Arc::new(Default::default());
                map.insert(key.clone(), Arc::downgrade(&m));
                m
            })
        };
        let guard = mtx.lock_owned().await;
        KeyedLockGuard {
            key: key.clone(),
            locks: self.clone(),
            guard: Some(guard),
        }
    }

    fn lock_map(&self) -> MutexGuard<HashMap<K, Weak<futures::lock::Mutex<()>>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<K: std::hash::Hash + Eq + Clone> Drop for KeyedLockGuard<K> {
    fn drop(&mut self) {
        // release the lock first decrementing the strong count
        self.guard.take();
        let mut map = self.locks.lock_map();
        if let Some(weak) = map.get(&self.key) {
            if weak.strong_count() == 0 {
                map.remove(&self.key);
            }
        }
    }
}
