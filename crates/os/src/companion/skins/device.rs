//! UI-owned skin activation. No resource is held in an interrupt mutex.
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use log::{info, warn};
use {crate::companion::render::skin::Skin, crate::companion::skins, crate::FoxControl};
static RELOAD: AtomicBool = AtomicBool::new(false);
// Only owned, unshared data crosses the task boundary. The UI creates its Rc.
type Prepared = (alloc::string::String, Skin, alloc::vec::Vec<skins::Choice>);
static READY: Channel<CriticalSectionRawMutex, Prepared, 1> = Channel::new();
pub async fn prepared(loaded: skins::Loaded) {
    let skin = alloc::rc::Rc::try_unwrap(loaded.skin).expect("freshly loaded skin is unshared");
    let choices = ::fs::global::with(|vfs| Ok(skins::choices(vfs)))
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    READY.send((loaded.id, skin, choices)).await;
}
pub fn activate_prepared(
    control: &FoxControl,
) -> Option<(alloc::string::String, alloc::vec::Vec<skins::Choice>)> {
    let (id, skin, choices) = READY.try_receive().ok()?;
    control.set_skin(alloc::rc::Rc::new(skin));
    info!("skin selected: {id} (prepared resource)");
    Some((id, choices))
}
pub fn request_reload() {
    crate::services::power::coordinator::wake();
    RELOAD.store(true, Ordering::Release);
}
pub fn reload_pending() -> bool {
    RELOAD.load(Ordering::Acquire)
}
fn restore() -> skins::Loaded {
    let loaded = ::fs::global::with(|vfs| Ok(skins::restore(vfs, &mut |reason| warn!("{reason}"))));
    match loaded {
        Ok(loaded) => {
            info!("skin selected: {}", loaded.id);
            loaded
        }
        Err(e) => {
            warn!("skin fallback default: VFS {e}");
            skins::Loaded {
                id: "default".into(),
                skin: Skin::builtin(),
            }
        }
    }
}

/// Boot/package reloads validate and decode resources on Core0 services. The
/// UI keeps drawing its placeholder/current skin until a complete one arrives.
#[embassy_executor::task]
pub async fn task() {
    while !crate::platform::storage::nor::ready() {
        embassy_time::Timer::after_millis(50).await;
    }
    let mut first = true;
    loop {
        if !crate::services::update::ota::device::active()
            && !crate::services::settings::device::busy()
            && READY.is_empty()
            && (first || RELOAD.swap(false, Ordering::AcqRel))
        {
            prepared(restore()).await;
            first = false;
        }
        embassy_time::Timer::after_millis(50).await;
    }
}
