//! USB-to-UI brain requests. VM work stays on the UI task; the console waits
//! for validation, durable commit and the actual runtime switch before replying.
use alloc::{format, string::String};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use critical_section::Mutex;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use log::{info, warn};
use {
    crate::companion::brain::store as brain_store, crate::companion::brain::store::Action,
    crate::companion::brain::DEFAULT_SOURCE, crate::formats::crc32::crc32, crate::Runtime,
};

static BUSY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<RefCell<Option<Action>>> = Mutex::new(RefCell::new(None));
static DONE: Signal<CriticalSectionRawMutex, Result<String, String>> = Signal::new();

async fn read_selected() -> Result<Option<String>, String> {
    crate::platform::runtime::executors::services::run(|| {
        ::fs::global::with(|vfs| Ok(brain_store::selected(vfs)))
    })
    .await
    .map_err(|e| format!("brain filesystem: {e}"))?
}
async fn persist(source: Option<String>) -> Result<(), String> {
    crate::platform::runtime::executors::services::run(move || {
        ::fs::global::with(|vfs| Ok(brain_store::persist(vfs, source.as_deref())))
    })
    .await
    .map_err(|e| format!("brain filesystem: {e}"))?
}

pub async fn boot(
    runtime: &mut Runtime,
    now: u64,
    allocator: &'static lua_engine::RawAllocator,
    clock: Option<fn() -> u64>,
) {
    let selected = read_selected().await;
    if matches!(&selected, Ok(None))
        && runtime.brain_active()
        && runtime.brain_source() == Some(DEFAULT_SOURCE)
    {
        // The bundled brain starts before storage and has already consumed
        // boot sensor edges. An absent override must not reset its state.
        info!("brain: bundled source retained after storage mount");
        return;
    }
    let source = match &selected {
        Ok(Some(source)) => source.as_str(),
        Ok(None) => DEFAULT_SOURCE,
        Err(e) => {
            warn!(
                "brain: stored source unavailable: {}",
                brain_store::error_line(e)
            );
            // Configure the inactive controller so a later valid USB replacement
            // can recover, while this boot uses the warm native behavior.
            "error('persisted brain source unreadable')"
        }
    };
    match runtime.enable_brain(source, now, allocator, clock) {
        Ok(()) => info!(
            "brain: resident Lua active crc={:08x}",
            crc32(source.as_bytes())
        ),
        Err(e) => warn!("brain: native fallback: {}", brain_store::error_line(&e)),
    }
}

pub async fn execute(action: Action) -> Result<String, String> {
    BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| String::from("brain operation busy"))?;
    DONE.reset();
    critical_section::with(|cs| *PENDING.borrow_ref_mut(cs) = Some(action));
    let result =
        match embassy_futures::select::select(DONE.wait(), embassy_time::Timer::after_secs(10))
            .await
        {
            embassy_futures::select::Either::First(result) => result,
            embassy_futures::select::Either::Second(()) => {
                critical_section::with(|cs| PENDING.borrow_ref_mut(cs).take());
                Err("brain UI service unavailable".into())
            }
        };
    BUSY.store(false, Ordering::Release);
    result
}

fn status(runtime: &Runtime) -> String {
    let source = runtime.brain_source().unwrap_or("");
    let selected = if source == DEFAULT_SOURCE {
        "default"
    } else {
        "custom"
    };
    format!(
        "active={} source={selected} bytes={} crc={:08x} error={}",
        if runtime.brain_active() {
            "lua"
        } else {
            "native"
        },
        source.len(),
        crc32(source.as_bytes()),
        runtime.brain_error().is_some() as u8
    )
}

pub async fn poll(runtime: &mut Runtime, now: u64, shutting_down: bool) {
    let Some(action) = critical_section::with(|cs| PENDING.borrow_ref_mut(cs).take()) else {
        return;
    };
    let verb = action.verb();
    let result = if shutting_down && action != Action::Status {
        Err("shutdown in progress".into())
    } else {
        match action {
            Action::Status => Ok(status(runtime)),
            Action::Install(source) => runtime
                .reload_brain_transaction_async(&source, now, async || {
                    persist(Some(source.clone())).await
                })
                .await
                .map(|()| String::new()),
            Action::Default => runtime
                .reload_brain_transaction_async(DEFAULT_SOURCE, now, async || persist(None).await)
                .await
                .map(|()| String::new()),
            Action::Reload => read_selected()
                .await
                .and_then(|source| {
                    runtime.reload_brain_transaction(
                        source.as_deref().unwrap_or(DEFAULT_SOURCE),
                        now,
                        || Ok(()),
                    )
                })
                .map(|()| String::new()),
        }
    };
    if let Err(e) = &result {
        warn!("brain: {verb} rejected: {}", brain_store::error_line(e));
    }
    DONE.signal(result);
}
