//! Settings operations and preference I/O run outside the LVGL owner.
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::select::{Either, select};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use os::hub::{
    Settings,
    settings::{self, Action},
};

static REQUEST: Channel<CriticalSectionRawMutex, Action, 1> = Channel::new();
static BUSY: AtomicBool = AtomicBool::new(false);
static RESULT: AtomicU32 = AtomicU32::new(0);
static UPDATE: Signal<CriticalSectionRawMutex, Settings> = Signal::new();
static REFRESH_STORAGE: AtomicBool = AtomicBool::new(false);
static FEEDBACK_PAINTED: Signal<CriticalSectionRawMutex, u32> = Signal::new();
static SKIN_FEEDBACK_PAINTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static PROXY_CHANGE: Signal<CriticalSectionRawMutex, Option<os::update::HttpProxy>> = Signal::new();
pub fn configure_proxy(proxy: Option<os::update::HttpProxy>) { PROXY_CHANGE.signal(proxy); }
pub fn take_proxy_change() -> Option<Option<os::update::HttpProxy>> { PROXY_CHANGE.try_take() }

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
                let scan = matches!(command, os::radio::Command::WifiOn);
                let response = super::radio::execute(command).await;
                if scan && response.starts_with("@ok") {
                    super::radio::execute(os::radio::Command::WifiScan).await
                } else {
                    response
                }
            }
            Action::ScreenOff => {
                Timer::after_millis(500).await;
                super::power::execute(os::power::Command::ScreenOff(30)).await
            }
            Action::Update(command) => {
                if super::update::submit(command) {"@ok update queued".into()} else {"@err update busy".into()}
            }
            Action::SelectSkin(id) => {
                SKIN_FEEDBACK_PAINTED.wait().await;
                if crate::ota_runtime::active() {
                    "@err skin OTA busy".into()
                } else {
                    let started = Instant::now();
                    let selected = ::fs::global::with(|vfs| Ok(os::hub::skins::select_loaded(vfs, &id, &mut |reason| log::warn!("{reason}"))));
                    log::info!("settings: skin {id} load_ms={}", started.elapsed().as_millis());
                    match selected {
                        Ok(Ok(loaded)) => { super::skins::prepared(loaded).await; "@ok skin selected".into() }
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
    let started = Instant::now();
    if let Ok(space) = ::fs::global::with(|vfs| vfs.space("/data")) {
        state.storage_ready = true;
        state.total_kib = (u64::from(space.blocks) * u64::from(space.block_size) / 1024) as u32;
        state.free_kib = (u64::from(space.free) * u64::from(space.block_size) / 1024) as u32;
    } else {
        state.storage_ready = false;
    }
    log::info!(
        "settings: space ready={} elapsed_us={}",
        state.storage_ready,
        started.elapsed().as_micros()
    );
}

/// Save the final slider position after a quiet interval. Network association
/// never holds up this independent task.
#[embassy_executor::task]
pub async fn preferences_task() {
    while !crate::drivers::storage::ready() {
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
                if !crate::ota_runtime::active() && REFRESH_STORAGE.swap(false, Ordering::AcqRel) {
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
        while crate::ota_runtime::active() {
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
        let started = Instant::now();
        while FEEDBACK_PAINTED.wait().await != revision {}
        let feedback_us = started.elapsed().as_micros();
        let mut save = settings::Save::new(value);
        let mut io_us = 0;
        let mut step_max_us = 0;
        let result = loop {
            while crate::ota_runtime::active() {
                Timer::after_millis(100).await;
            }
            let step = Instant::now();
            let result = ::fs::global::with(|vfs| save.step(vfs));
            let elapsed = step.elapsed().as_micros();
            io_us += elapsed;
            step_max_us = step_max_us.max(elapsed);
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
                log::info!(
                    "settings: preferences saved io_us={io_us} step_max_us={step_max_us} feedback_us={feedback_us} wall_us={}",
                    started.elapsed().as_micros()
                );
            }
            Err(error) => {
                state.error = true;
                log::warn!("settings: save failed {error}");
            }
        }
        publish(state);
    }
}
