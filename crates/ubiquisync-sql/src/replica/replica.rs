use ubiquisync_core::{crypto::credentials::Credentials, ids::PeerId};

pub struct Replica<D> {
    self_id: PeerId,
    credentials: Box<dyn Credentials>,
    db: D,
}
