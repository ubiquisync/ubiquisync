use crate::pack::FileRemote;

pub struct PackStore {
    remote: Box<dyn FileRemote>,
}
