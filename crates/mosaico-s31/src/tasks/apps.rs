//! The Lua app registry on the device, and requests to the UI task about it.
//!
//! The USB push protocol (`console`) changes `/data/apps` and then asks the
//! UI task, which owns the launcher, to rescan, to open an app or to leave
//! the running one. The UI task polls [`take_request`] once per loop.

use core::cell::RefCell;

use alloc::string::String;
use critical_section::Mutex;
use log::{info, warn};
use mosaico_s31::hub::registry::{IconCache, Registry};

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
    critical_section::with(|cs| {
        let mut p = PENDING.borrow_ref_mut(cs);
        p.reload |= reload;
        if action.is_some() {
            p.action = action;
        }
    });
}

pub fn take_request() -> Option<Request> {
    critical_section::with(|cs| {
        let req = core::mem::take(&mut *PENDING.borrow_ref_mut(cs));
        (req.reload || req.action.is_some()).then_some(req)
    })
}

/// The first scan, guarded by the crash record (see `crashlog`).
pub fn scan_at_boot(icons: &mut IconCache) -> Registry {
    use crate::crashlog::{enter, skip, Stage};
    if skip(Stage::AppScan) {
        warn!("apps: last boot panicked while scanning; built-ins only this boot");
        return Registry::builtin_only(icons);
    }
    enter(Stage::AppScan);
    let reg = scan(icons);
    enter(Stage::Running);
    reg
}

/// Scan `/data/apps`, `/system/apps` and the built-ins. Without a file system
/// only the built-ins are listed.
pub fn scan(icons: &mut IconCache) -> Registry {
    let started = embassy_time::Instant::now();
    let scanned = mosaico_fs::global::with(|vfs| {
        Ok(Registry::scan(vfs, icons, &mut |line| {
            warn!("apps: {line}")
        }))
    });
    let reg = match scanned {
        Ok(reg) => reg,
        Err(e) => {
            warn!("apps: file system unavailable ({e}), built-ins only");
            Registry::builtin_only(icons)
        }
    };
    let mut names = String::new();
    for a in &reg.apps {
        names.push(' ');
        names.push_str(&a.app.id);
        if !a.runnable() {
            names.push('*');
        }
    }
    info!(
        "apps: {} in {} ms:{names} (* = needs missing modules)",
        reg.apps.len(),
        started.elapsed().as_millis()
    );
    reg
}
