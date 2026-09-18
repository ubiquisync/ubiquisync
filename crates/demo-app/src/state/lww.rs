use std::{cmp::Ordering, collections::HashMap, ops::Deref};

use dioxus::signals::{ReadableExt, Signal, WritableExt};
use ubiquisync_core::{hlc::Timestamp, uuid::Uuid};

#[derive(Default)]
pub struct Lww<T: Default + Ord + 'static> {
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
        match self.timestamp.cmp(&ts) {
            Ordering::Less => {
                self.value.set(value);
                self.timestamp = ts;
            }
            Ordering::Equal => {
                if self.value.peek().lt(&value) {
                    self.value.set(value);
                }
            }
            Ordering::Greater => {}
        }
    }
}

impl<T: Default + Ord + 'static> Lww<T> {
    /// Read a value and subscribe to change updates.
    pub fn read(&self) -> impl Deref<Target = T> + '_ {
        self.value.read()
    }

    /// Read a value without subscribing to change updates.
    pub fn peek(&self) -> impl Deref<Target = T> + '_ {
        self.value.peek()
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

impl<T: Apply + Default> Apply for HashMap<Uuid, T> {
    type Op = (Uuid, Vec<T::Op>);

    fn apply(&mut self, ts: Timestamp, op: Self::Op) {
        let e = self.entry(op.0).or_default();
        for o in op.1 {
            e.apply(ts, o);
        }
    }
}

#[macro_export]
macro_rules! def_state {
    ($name:ident { $($f_name:ident: $f_type:ty),* $(,)?}) => {
        pastey::paste! {
            #[derive(Default)]
            pub struct $name {
                $(pub $f_name: $f_type),*
            }

            #[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
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
