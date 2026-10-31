//! The Lua app registry on the device, and requests to the UI task about it.
//!
//! The USB push protocol (`console`) changes `/data/apps` and then asks the
//! UI task, which owns the launcher, to rescan, to open an app or to leave
//! the running one. The UI task polls [`take_request`] once per loop.

use core::cell::RefCell;

use alloc::string::String;
use critical_section::Mutex;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use log::warn;
use {crate::apps::registry::IconCache, crate::apps::registry::Registry};
static SCAN: Channel<CriticalSectionRawMutex, bool, 1> = Channel::new();
static SCANNED: Channel<CriticalSectionRawMutex, Registry, 1> = Channel::new();
pub fn request_scan(boot: bool) {
    SCAN.try_send(boot).expect("one registry scan in flight");
}
pub fn take_scanned() -> Option<Registry> {
    SCANNED.try_receive().ok()
}

/// Registry traversal and JPEG decoding belong to services. The UI keeps
/// rendering while a complete owned registry is prepared; no LVGL calls here.
#[embassy_executor::task]
pub async fn scan_task() {
    let mut icons = IconCache::new();
    loop {
        let boot = SCAN.receive().await;
        let registry = if boot {
            scan_at_boot(&mut icons)
        } else {
            scan(&mut icons)
        };
        SCANNED.send(registry).await;
    }
}

/// What to do with the running app after an optional rescan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open the app with this id.
    Run(String),
    /// Leave the running Lua app, back to the launcher.
    Stop,
}

/// Work for the UI task. Requests accumulate until it takes them: the rescan
/// flag is sticky, the latest action wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// Rescan the roots. A running app whose id is still listed restarts with
    /// its new files.
    pub reload: bool,
    pub action: Option<Action>,
}

static PENDING: Mutex<RefCell<Request>> = Mutex::new(RefCell::new(Request {
    reload: false,
    action: None,
}));

pub fn request(reload: bool, action: Option<Action>) {
    crate::services::power::coordinator::wake();
    critical_section::with(|cs| {
        let mut p = PENDING.borrow_ref_mut(cs);
        p.reload |= reload;
        if action.is_some() && !is_managing() {
            p.action = action;
        }
    });
}

pub fn has_request() -> bool {
    critical_section::with(|cs| {
        let pending = PENDING.borrow_ref(cs);
        pending.reload || pending.action.is_some()
    })
}

pub fn take_request() -> Option<Request> {
    critical_section::with(|cs| {
        let mut req = core::mem::take(&mut *PENDING.borrow_ref_mut(cs));
        if *MANAGING.borrow_ref(cs) == crate::apps::lifecycle::uninstall::Phase::AwaitingRefresh {
            req.reload = true;
            req.action = Some(Action::Stop);
        }
        (req.reload || req.action.is_some()).then_some(req)
    })
}

/// The first scan, guarded by the crash record (see `crashlog`).
pub fn scan_at_boot(icons: &mut IconCache) -> Registry {
    use {
        crate::platform::runtime::crashlog::enter, crate::platform::runtime::crashlog::skip,
        crate::platform::runtime::crashlog::Stage,
    };
    if skip(Stage::AppScan) {
        warn!("apps: last boot panicked while scanning; Rust apps only this boot");
        return Registry::default();
    }
    enter(Stage::AppScan);
    let recovery = ::fs::global::with(|vfs| {
        crate::apps::lifecycle::uninstall::recover(vfs).map_err(|e| {
            warn!("apps: recovery: {e}");
            ::fs::Errno::IO
        })
    });
    if let Err(e) = recovery {
        warn!("apps: recovery failed ({e}); Rust apps only");
        enter(Stage::Running);
        return Registry::default();
    }
    let reg = scan(icons);
    enter(Stage::Running);
    reg
}

/// Scan `/data/apps`, `/system/apps` and the built-ins. Without a file system
/// only the built-ins are listed.
pub fn scan(icons: &mut IconCache) -> Registry {
    let scanned = ::fs::global::with(|vfs| {
        Ok(Registry::scan(vfs, icons, &mut |line| {
            warn!("apps: {line}")
        }))
    });
    let reg = match scanned {
        Ok(reg) => reg,
        Err(e) => {
            warn!("apps: file system unavailable ({e}), Rust apps only");
            Registry::default()
        }
    };
    reg
}

static MANAGING: Mutex<RefCell<crate::apps::lifecycle::uninstall::Phase>> =
    Mutex::new(RefCell::new(crate::apps::lifecycle::uninstall::Phase::Idle));
static UI_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub fn set_ui_management_pending(pending: bool) {
    UI_PENDING.store(pending, core::sync::atomic::Ordering::Release);
}
pub fn is_managing() -> bool {
    UI_PENDING.load(core::sync::atomic::Ordering::Acquire)
        || critical_section::with(|cs| MANAGING.borrow_ref(cs).busy())
}
pub fn acknowledge_management_refresh() {
    critical_section::with(|cs| {
        let mut phase = MANAGING.borrow_ref_mut(cs);
        if *phase == crate::apps::lifecycle::uninstall::Phase::AwaitingRefresh {
            phase.acknowledge_refresh();
            crate::apps::lua::task::release_management();
        }
    });
}
/// A second owner gets busy. The start barrier remains held through UI rescan.
pub async fn manage(op: crate::apps::lifecycle::uninstall::Operation) -> Result<(), String> {
    if crate::services::update::task::busy() {
        return Err("系统更新进行中".into());
    }
    critical_section::with(|cs| MANAGING.borrow_ref_mut(cs).acquire()).map_err(String::from)?;
    crate::apps::lua::task::quiesce().await;
    let result = crate::platform::runtime::executors::services::run(move || {
        ::fs::global::with(|vfs| Ok(crate::apps::lifecycle::uninstall::execute(vfs, op)))
    })
    .await
    .map_err(|e| alloc::format!("{e}"))
    .and_then(|r| r);
    critical_section::with(|cs| MANAGING.borrow_ref_mut(cs).completed());
    result
}
