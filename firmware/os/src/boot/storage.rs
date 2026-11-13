//! Mounted-volume application recovery and directory preparation.
use fs::global;
use log::warn;
/// Runs after the NAND /data mount is available, including explicit format.
pub fn finish_data_init() {
    match global::with(|v| Ok(crate::apps::lifecycle::uninstall::recover(v))) {
        Ok(Ok(())) => {}
        other => warn!("fs: app recovery incomplete: {other:?}; pending identities remain blocked"),
    }
    // littlefs free-space accounting traverses every allocated file block.
    // It belongs to the Storage page, never the resource readiness barrier.
    let _ = global::with(|v| v.create_dir_all("/data/apps"));
}
