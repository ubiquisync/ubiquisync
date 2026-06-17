use ubiquisync_core::hlc::Timestamp;
use ubiquisync_core::sync::SyncError;

pub fn format_timestamp(ts: Timestamp) -> Result<String, SyncError> {
    let millis = ts.millis();
    // Keep the failing value and jiff's reason in the error — a bare "invalid
    // timestamp" gives nothing to diagnose bad data with.
    let jts = jiff::Timestamp::from_millisecond(millis as i64)
        .map_err(|e| SyncError::EncodingError(format!("invalid timestamp {millis} ms: {e}")))?;
    Ok(jts.strftime("%Y%m%d%H%M%S").to_string())
}
