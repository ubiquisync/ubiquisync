use std::{
    cmp::Ordering,
    collections::HashMap,
    ops::Deref,
    sync::{Arc, RwLock, RwLockReadGuard},
};

use dioxus::{
    signals::{ReadSignal, ReadableExt, Signal, WritableExt},
    stores::Store,
};
use ubiquisync_core::{hlc::Timestamp, uuid::Uuid};

use crate::state::Init;

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

impl<T: Default + Ord + 'static> Lww<T> {
    /// Read value and subscribe to change updates.
    pub fn read(&self) -> impl Deref<Target = T> + '_ {
        self.signal().read_unchecked()
    }

    /// Read value without subscribing to change updates.
    pub fn peek(&self) -> impl Deref<Target = T> + '_ {
        self.signal().peek_unchecked()
    }

    pub fn signal(&self) -> ReadSignal<T> {
        self.read_guard().value.into()
    }

    pub fn timestamp(&self) -> Timestamp {
        self.read_guard().timestamp
    }

    fn read_guard(&self) -> RwLockReadGuard<'_, LwwInner<T>> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: Default + Ord + 'static> Init for Lww<T> {
    fn init() -> Self {
        Default::default()
    }
}

impl<T: Default + Ord + Clone + 'static> PartialEq for Lww<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T: Apply + Init + 'static> Apply for Store<HashMap<Uuid, T>> {
    type Op = (Uuid, Vec<T::Op>);

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        let mut guard = self.write();
        let e = guard.entry(op.0).or_insert_with(T::init);
        for o in op.1 {
            e.apply(ts, o);
        }
    }
}

#[macro_export]
macro_rules! def_state {
    ($name:ident { $($f_name:ident: $f_type:ty),* $(,)?}) => {
        pastey::paste! {
            #[derive(Clone, PartialEq)]
            pub struct $name {
                $(pub $f_name: $f_type),*
            }

            #[derive(Debug, Clone, PartialEq, Eq, borsh::BorshSerialize, borsh::BorshDeserialize)]
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

            impl $crate::state::Init for $name {
                fn init() -> Self {
                    Self {
                        $($f_name: <$f_type as $crate::state::Init>::init()),*
                    }

                }
            }
        }
    };

}
