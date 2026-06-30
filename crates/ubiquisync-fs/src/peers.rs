use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ubiquisync_core::uuid::Uuid;

/// List peer IDs by scanning base64-encoded subdirectories.
pub fn list_peers(root: &Path) -> Vec<Uuid> {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            // Decode to bytes, then coerce into a fixed-size Uuid array.
            // Names that aren't valid base64 or aren't 16 bytes are skipped.
            let bytes = URL_SAFE_NO_PAD.decode(e.file_name().to_str()?).ok()?;
            bytes.try_into().ok()
        })
        .collect()
}

pub fn peer_dir(root: &Path, peer_id: &Uuid) -> PathBuf {
    root.join(URL_SAFE_NO_PAD.encode(peer_id))
}
