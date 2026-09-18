use std::collections::HashMap;

use dioxus::stores::Store;
use ubiquisync_core::uuid::Uuid;

pub trait Init {
    fn init() -> Self;
}

impl<T: 'static> Init for Store<HashMap<Uuid, T>> {
    fn init() -> Self {
        Store::new(HashMap::new())
    }
}
