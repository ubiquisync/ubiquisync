use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{Arc, RwLock, RwLockReadGuard},
};

use dioxus::signals::{ReadSignal, ReadableExt, Signal, WritableExt};
use ubiquisync_core::{hlc::Timestamp, uuid::Uuid};

#[derive(Default, Clone)]
pub struct Lww<T: Default + Ord + 'static>(Arc<RwLock<LwwInner<T>>>);

#[derive(Default)]
struct LwwInner<T: Default + Ord + 'static> {
    value: Signal<T>,
    timestamp: Timestamp,
}

pub trait Apply {
    type Op;

    fn apply(&mut self, ts: Timestamp, op: Self::Op);
}

impl<T: Default + Ord + 'static> Apply for Lww<T> {
    type Op = T;

    fn apply(&mut self, ts: Timestamp, value: Self::Op) {
        let mut guard = self.0.write().unwrap_or_else(|e| e.into_inner());
        match guard.timestamp.cmp(&ts) {
            Ordering::Less => {
                guard.value.set(value);
                guard.timestamp = ts;
            }
            Ordering::Equal => {
                if guard.value.peek().lt(&value) {
                    guard.value.set(value);
                }
            }
            Ordering::Greater => {}
        }
    }
}

impl<T: Default + Ord + Clone + 'static> Lww<T> {
    /// Read a value and subscribe to change updates.
    pub fn read(&self) -> ReadSignal<T> {
        self.read_guard().value.into()
    }

    /// Read a value without subscribing to change updates.
    pub fn peek(&self) -> T {
        self.read_guard().value.peek().clone()
    }

    pub fn timestamp(&self) -> Timestamp {
        self.read_guard().timestamp
    }

    fn read_guard(&self) -> RwLockReadGuard<LwwInner<T>> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }
}

pub type StateMap<T> = Arc<RwLock<HashMap<Uuid, T>>>;

impl<T: Apply + Default + Clone> Apply for StateMap<T> {
    type Op = (Uuid, Vec<T::Op>);

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        let mut guard = self.write().unwrap_or_else(|e| e.into_inner());
        let e = guard.entry(op.0).or_default();
        for o in op.1 {
            e.apply(ts, o);
        }
    }
}

impl<T: Apply + Default + 'static> Apply for Signal<HashMap<Uuid, T>> {
    type Op = (Uuid, Vec<T::Op>);

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        let mut guard = self.write();
        let e = guard.entry(op.0).or_default();
        for o in op.1 {
            e.apply(ts, o);
        }
    }
}

#[macro_export]
macro_rules! def_state {
    ($name:ident { $($f_name:ident: $f_type:ty),* $(,)?}) => {
        pastey::paste! {
            #[derive(Default, Clone)]
            pub struct $name {
                $(pub $f_name: $f_type),*
            }

            #[derive(Debug, Clone, borsh::BorshSerialize, borsh::BorshDeserialize)]
            #[cfg_attr(test, derive(test_strategy::Arbitrary))]
            #[borsh(use_discriminant = true)]
            #[repr(u8)]
            pub enum [< $name Op >] {
                $([< $f_name:camel >](<$f_type as $crate::state::lww::Apply>::Op)),*
            }

            impl $crate::state::lww::Apply for $name {
                type Op = [< $name Op>];

                fn apply(&mut self, ts: ubiquisync_core::hlc::Timestamp, op: Self::Op) {
                    match op {
                        $(Self::Op::[< $f_name:camel >](x) => self.$f_name.apply(ts, x)),*
                    }
                }
            }
        }
    };

}
