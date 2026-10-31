//! Settings operations and preference I/O run outside the LVGL owner.
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Timer};
use {
    crate::services::settings::preferences as settings,
    crate::services::settings::preferences::Settings, crate::services::settings::Command as Action,
};

static REQUEST: Channel<CriticalSectionRawMutex, Action, 1> = Channel::new();
static BUSY: AtomicBool = AtomicBool::new(false);
static RESULT: AtomicU32 = AtomicU32::new(0);
static UPDATE: Signal<CriticalSectionRawMutex, Settings> = Signal::new();
static REFRESH_STORAGE: AtomicBool = AtomicBool::new(false);
static REFRESH_HEAP: AtomicBool = AtomicBool::new(false);
static FEEDBACK_PAINTED: Signal<CriticalSectionRawMutex, u32> = Signal::new();
static SKIN_FEEDBACK_PAINTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static PROXY_CHANGE: Signal<CriticalSectionRawMutex, Option<crate::services::update::HttpProxy>> =
    Signal::new();
pub fn configure_proxy(proxy: Option<crate::services::update::HttpProxy>) {
    PROXY_CHANGE.signal(proxy);
}
pub fn take_proxy_change() -> Option<Option<crate::services::update::HttpProxy>> {
    PROXY_CHANGE.try_take()
}

#[derive(Clone, Copy, Default)]
pub struct Preferences {
    pub loaded: bool,
    pub initial: Option<Settings>,
    pub persisted: Option<Settings>,
    pub error: bool,
    pub saving_revision: u32,
    pub storage_ready: bool,
    pub total_kib: u32,
    pub free_kib: u32,
    pub heap_free_kib: Option<u32>,
}
static PREFERENCES: critical_section::Mutex<Cell<Preferences>> =
    critical_section::Mutex::new(Cell::new(Preferences {
        loaded: false,
        initial: None,
        persisted: None,
        error: false,
        saving_revision: 0,
        storage_ready: false,
        total_kib: 0,
        free_kib: 0,
        heap_free_kib: None,
    }));
pub fn preferences() -> Preferences {
    critical_section::with(|cs| PREFERENCES.borrow(cs).get())
}
fn publish(value: Preferences) {
    critical_section::with(|cs| PREFERENCES.borrow(cs).set(value));
}
pub fn save(value: Settings) {
    UPDATE.signal(value);
}
pub fn refresh_storage() {
    REFRESH_STORAGE.store(true, Ordering::Release);
}
/// Coalesce About-page observations on the existing Core0 services executor.
/// Reading the allocator must not hold up the LVGL owner on Core1.
pub fn refresh_heap() {
    REFRESH_HEAP.store(true, Ordering::Release);
}
/// Called after the saving indicator's frame is uploaded, or when Settings
/// is no longer visible. The revision prevents an older frame releasing a save.
pub fn feedback_painted(revision: u32) {
    if revision != 0 {
        FEEDBACK_PAINTED.signal(revision);
    }
}
pub fn skin_feedback_painted() {
    SKIN_FEEDBACK_PAINTED.signal(());
}
pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}
pub fn result() -> u32 {
    RESULT.load(Ordering::Acquire)
}
pub fn submit(action: Action) -> bool {
    if BUSY.swap(true, Ordering::AcqRel) {
        return false;
    }
    if matches!(&action, Action::SelectSkin(_)) {
        SKIN_FEEDBACK_PAINTED.reset();
    }
    if REQUEST.try_send(action).is_err() {
        BUSY.store(false, Ordering::Release);
        false
    } else {
        true
    }
}

#[embassy_executor::task]
pub async fn task() {
    loop {
        let action = REQUEST.receive().await;
        let response = match action {
            Action::Radio(command) => {
                let scan = matches!(command, crate::services::radio::Command::WifiOn);
                let response = crate::services::radio::device::execute(command).await;
                if scan && response.starts_with("@ok") {
                    crate::services::radio::device::execute(
                        crate::services::radio::Command::WifiScan,
                    )
                    .await
                } else {
                    response
                }
            }
            Action::ScreenOff => {
                Timer::after_millis(500).await;
                crate::services::power::coordinator::execute(
                    crate::services::power::standby::Command::ScreenOff(30),
                )
                .await
            }
            Action::Update(command) => {
                if crate::services::update::task::submit(command) {
                    "@ok update queued".into()
                } else {
                    "@err update busy".into()
                }
            }
            Action::SelectSkin(id) => {
                SKIN_FEEDBACK_PAINTED.wait().await;
                if crate::services::update::ota::device::active() {
                    "@err skin OTA busy".into()
                } else {
                    let selected = ::fs::global::with(|vfs| {
                        Ok(crate::companion::skins::select_loaded(
                            vfs,
                            &id,
                            &mut |reason| log::warn!("{reason}"),
                        ))
                    });
                    match selected {
                        Ok(Ok(loaded)) => {
                            crate::companion::skins::device::prepared(loaded).await;
                            "@ok skin selected".into()
                        }
                        _ => "@err skin selection failed".into(),
                    }
                }
            }
        };
        let revision = RESULT.load(Ordering::Relaxed).wrapping_add(2) & !1;
        RESULT.store(
            revision | u32::from(!response.starts_with("@ok")),
            Ordering::Release,
        );
        BUSY.store(false, Ordering::Release);
    }
}

fn read_space(state: &mut Preferences) {
    if let Ok(space) = ::fs::global::with(|vfs| vfs.space("/data")) {
        state.storage_ready = true;
        state.total_kib = (u64::from(space.blocks) * u64::from(space.block_size) / 1024) as u32;
        state.free_kib = (u64::from(space.free) * u64::from(space.block_size) / 1024) as u32;
    } else {
        state.storage_ready = false;
    }
}

/// Save the final slider position after a quiet interval. Network association
/// never holds up this independent task.
#[embassy_executor::task]
pub async fn preferences_task() {
    while !crate::platform::storage::nor::ready() {
        Timer::after_millis(50).await;
    }
    let mut state = preferences();
    match ::fs::global::with(settings::load) {
        Ok(initial) => {
            state.initial = initial;
            state.persisted = initial;
        }
        Err(error) => {
            state.error = true;
            log::warn!("settings: load failed {error}");
        }
    }
    state.loaded = true;
    read_space(&mut state);
    publish(state);
    let mut revision = 0u32;
    loop {
        let mut value = match select(UPDATE.wait(), Timer::after_millis(250)).await {
            Either::First(value) => value,
            Either::Second(()) => {
                if REFRESH_HEAP.swap(false, Ordering::AcqRel) {
                    state.heap_free_kib = Some((esp_alloc::HEAP.free() / 1024) as u32);
                    publish(state);
                }
                if !crate::services::update::ota::device::active()
                    && REFRESH_STORAGE.swap(false, Ordering::AcqRel)
                {
                    read_space(&mut state);
                    publish(state);
                }
                continue;
            }
        };
        loop {
            match select(UPDATE.wait(), Timer::after(Duration::from_millis(600))).await {
                Either::First(next) => value = next,
                Either::Second(()) => break,
            }
        }
        while crate::services::update::ota::device::active() {
            Timer::after_millis(100).await;
        }
        if let Some(next) = UPDATE.try_take() {
            value = next;
        }
        if state.persisted == Some(value) {
            state.error = false;
            publish(state);
            continue;
        }
        revision = revision.wrapping_add(1).max(1);
        FEEDBACK_PAINTED.reset();
        state.saving_revision = revision;
        state.error = false;
        publish(state);
        while FEEDBACK_PAINTED.wait().await != revision {}
        let mut save = settings::Save::new(value);
        let result = loop {
            while crate::services::update::ota::device::active() {
                Timer::after_millis(100).await;
            }
            let result = ::fs::global::with(|vfs| save.step(vfs));
            match result {
                Ok(true) => break Ok(()),
                Ok(false) => embassy_futures::yield_now().await,
                Err(error) => break Err(error),
            }
        };
        state.saving_revision = 0;
        match result {
            Ok(()) => {
                state.persisted = Some(value);
                state.error = false;
            }
            Err(error) => {
                state.error = true;
                log::warn!("settings: save failed {error}");
            }
        }
        publish(state);
    }
}
