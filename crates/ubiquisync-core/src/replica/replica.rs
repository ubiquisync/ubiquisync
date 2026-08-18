use std::sync::Mutex;

use crate::{crypto::credentials::Credentials, hlc::Hlc, ids::PeerId, reducer::ReducerResolver};

pub struct Replica<S, C> {
    self_id: PeerId,
    credentials: Box<dyn Credentials>,
    storage: S,
    auth_controller: C,
    hlc: Mutex<Hlc>,
    reducers: Box<dyn ReducerResolver>,
}
