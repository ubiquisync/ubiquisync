use ubiquisync_core::hlc::HlcService;

use crate::hlc_storage::SqlHlcStorage;

pub struct SqlLogProcessor<R> {
    r: R,
    hlc: HlcService<SqlHlcStorage>,
}
