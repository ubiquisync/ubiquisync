use ubiquisync_core::hlc::Timestamp;
use ubiquisync_core::sync::SyncError;

pub fn format_timestamp(ts: Timestamp) -> Result<String, SyncError> {
    let jts = jiff::Timestamp::from_millisecond(ts.millis() as i64)
        .map_err(|_| SyncError::EncodingError("invalid timestamp".into()))?;
    Ok(jts.strftime("%Y%m%d%H%M%S").to_string())
}
