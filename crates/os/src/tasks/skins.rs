//! UI-owned skin activation. No resource is held in an interrupt mutex.
use core::sync::atomic::{AtomicBool, Ordering};
use log::{info, warn};
use os::{fox::skin::Skin, hub::skins, FoxControl};
static RELOAD: AtomicBool = AtomicBool::new(false);
pub fn request_reload() {
    RELOAD.store(true, Ordering::Release);
}
pub fn take_reload() -> bool {
    RELOAD.swap(false, Ordering::AcqRel)
}
pub fn apply(control: &FoxControl) {
    let loaded = ::fs::global::with(|vfs| Ok(skins::restore(vfs, &mut |reason| warn!("{reason}"))));
    match loaded {
        Ok(loaded) => {
            info!("skin selected: {}", loaded.id);
            control.set_skin(loaded.skin);
        }
        Err(e) => {
            warn!("skin fallback default: VFS {e}");
            control.set_skin(Skin::builtin());
        }
    }
}
