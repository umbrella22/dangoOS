//! Mounted-volume application recovery and directory preparation.
use fs::global;
use log::{info, warn};
/// Runs after the NAND /data mount is available, including explicit format.
pub fn finish_data_init() {
    match global::with(|v| Ok(crate::apps::lifecycle::uninstall::recover(v))) {
        Ok(Ok(())) => {}
        other => warn!("fs: app recovery incomplete: {other:?}; pending identities remain blocked"),
    }
    let _ = global::with(|v| {
        for dir in ["/system", "/data"] {
            if let Ok(s) = v.space(dir) {
                info!("fs: {dir} {}/{} blocks used", s.blocks - s.free, s.blocks);
            }
        }
        v.create_dir_all("/data/apps")
    });
}
