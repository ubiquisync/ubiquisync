use ubiquisync_core::hlc::Timestamp;

pub struct LwwRegister<V> {
    timestamp: Timestamp,
    value: V,
}
