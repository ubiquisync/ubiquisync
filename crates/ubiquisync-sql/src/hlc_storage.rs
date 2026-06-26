use ubiquisync_core::hlc::HlcStorage;

use crate::db::DbError;

pub struct SqlHlcStorage {}

impl SqlHlcStorage {
    pub fn new() {}
}

impl HlcStorage for SqlHlcStorage {
    type Error = DbError;

    fn load(&self) -> Result<Option<u64>, Self::Error> {
        todo!()
    }

    fn save(&self, raw: u64) -> Result<(), Self::Error> {
        todo!()
    }
}
