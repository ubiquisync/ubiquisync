use std::cmp::Ordering;

use ubiquisync_core::hlc::Timestamp;

#[derive(Default)]
pub struct Lww<T: Default + Clone + Ord> {
    value: T,
    timestamp: Timestamp,
}

pub trait Apply {
    type Op;

    fn apply(&mut self, ts: Timestamp, op: &Self::Op);
}

impl<T: Default + Ord + Clone> Apply for Lww<T> {
    type Op = T;

    fn apply(&mut self, ts: Timestamp, value: &Self::Op) {
        match self.timestamp.cmp(&ts) {
            Ordering::Less => {
                self.value = value.clone();
                self.timestamp = ts;
            }
            Ordering::Equal => {
                if self.value.lt(value) {
                    self.value = value.clone();
                }
            }
            Ordering::Greater => {}
        }
    }
}

impl<T: Default + Ord + Clone> Lww<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
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

            #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
            #[borsh(use_discriminant = true)]
            #[repr(u8)]
            pub enum [< $name Op >] {
                $([< $f_name:camel >](<$f_type as $crate::state::lww::Apply>::Op)),*
            }

            impl $crate::state::lww::Apply for $name {
                type Op = [< $name Op>];

                fn apply(&mut self, ts: ubiquisync_core::hlc::Timestamp, op: &Self::Op) {
                    match op {
                        $(Self::Op::[< $f_name:camel >](x) => self.$f_name.apply(ts, x)),*
                    }
                }
            }
        }
    };

}
