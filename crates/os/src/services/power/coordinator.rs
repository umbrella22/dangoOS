//! The UI owns standby admission and lifecycle; the plane owns panel commands.
//! No PMU, rail, clock, filesystem, or MCU sleep operations occur here.
use alloc::{format, string::String};
use core::{
    cell::Cell,
    sync::atomic::{AtomicU32, Ordering},
};
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Instant, Timer};
use {
    crate::services::display::status::{BRIGHTNESS_RESTORED, FIRST_FRAME},
    crate::services::power::wake::WakeCycle,
};
use {
    crate::services::power::standby::Command, crate::services::power::standby::Policy,
    crate::services::power::standby::State,
};

static WAKE_EPOCH: AtomicU32 = AtomicU32::new(0);
static NEXT_REQUEST: AtomicU32 = AtomicU32::new(1);
static ACTIVE_REQUEST: AtomicU32 = AtomicU32::new(0);
static STATE: critical_section::Mutex<Cell<State>> =
    critical_section::Mutex::new(Cell::new(State::Active));
static RESTORE: critical_section::Mutex<Cell<WakeCycle>> =
    critical_section::Mutex::new(Cell::new(WakeCycle::new()));
static PENDING: Signal<CriticalSectionRawMutex, Request> = Signal::new();
static DONE: Signal<CriticalSectionRawMutex, (u32, Result<(), &'static str>)> = Signal::new();

struct Request {
    id: u32,
    command: Command,
    wake_epoch: u32,
}

/// A generation, rather than a consumable edge: input during Sleep In survives
/// the panel acknowledgement and forces Sleep Out immediately afterwards.
pub fn wake() {
    WAKE_EPOCH.fetch_add(1, Ordering::AcqRel);
}
fn update_restore(f: impl FnOnce(&mut WakeCycle)) {
    critical_section::with(|cs| {
        let mut restore = RESTORE.borrow(cs).get();
        f(&mut restore);
        RESTORE.borrow(cs).set(restore);
    });
}
fn restore_cycle() -> WakeCycle {
    critical_section::with(|cs| RESTORE.borrow(cs).get())
}
pub fn cycle() -> u32 {
    restore_cycle().cycle
}
pub fn panel_ready(cycle: u32) {
    update_restore(|t| t.panel_ready(cycle));
}
pub fn frame_done(cycle: u32) {
    update_restore(|t| t.frame_done(cycle));
}
pub fn brightness_done(cycle: u32) {
    update_restore(|t| {
        t.brightness_done(cycle);
    });
}
fn snapshot() -> (State, WakeCycle) {
    critical_section::with(|cs| (STATE.borrow(cs).get(), RESTORE.borrow(cs).get()))
}
fn frame_ready_from(state: State, t: WakeCycle) -> bool {
    state == State::Active
        && if t.cycle == 0 {
            FIRST_FRAME.load(Ordering::Acquire) && BRIGHTNESS_RESTORED.load(Ordering::Acquire)
        } else {
            t.complete()
        }
}
fn frame_ready() -> bool {
    let (state, t) = snapshot();
    frame_ready_from(state, t)
}
fn state() -> State {
    critical_section::with(|cs| STATE.borrow(cs).get())
}
pub fn status() -> String {
    let (state, t) = snapshot();
    format!(
        "@ok power-status state={} mcu=running mode=manual cycle={} frame_ready={}",
        state.name(),
        t.cycle,
        frame_ready_from(state, t) as u8
    )
}

pub async fn execute(command: Command) -> String {
    if command == Command::Status {
        return status();
    }
    if command == Command::ScreenOn {
        wake();
    }
    match request(command).await {
        Ok(()) => format!("@ok {} state={}", command.verb(), state().name()),
        Err(error) => format!("@err {} {error}", command.verb()),
    }
}

async fn request(command: Command) -> Result<(), &'static str> {
    let id = loop {
        let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            break id;
        }
    };
    if ACTIVE_REQUEST
        .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("power service busy");
    }
    DONE.reset();
    PENDING.signal(Request {
        id,
        command,
        wake_epoch: WAKE_EPOCH.load(Ordering::Acquire),
    });
    let deadline = Instant::now() + embassy_time::Duration::from_secs(5);
    let result = loop {
        match select(DONE.wait(), Timer::at(deadline)).await {
            Either::First((reply_id, result)) if reply_id == id => break result,
            Either::First(_) => continue, // Late reply to a cancelled request.
            Either::Second(()) => {
                wake();
                break Err("power service timeout; wake requested");
            }
        }
    };
    let _ = ACTIVE_REQUEST.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
    result
}

/// Deferred operations such as OTA must wait for the restore acknowledgement.
pub async fn ensure_awake() -> bool {
    wake();
    if state() != State::Active && request(Command::ScreenOn).await.is_err() {
        return false;
    }
    // The UI must render after Sleep Out before OTA suspends it. Panel-command
    // acknowledgement alone leaves a window with brightness still at zero.
    let deadline = Instant::now() + embassy_time::Duration::from_secs(2);
    while state() == State::Active && !frame_ready() {
        if Instant::now() >= deadline {
            return false;
        }
        Timer::after_millis(5).await;
    }
    frame_ready()
}

pub struct Coordinator {
    policy: Policy,
    restore_haptics: bool,
    last_epoch: u32,
}
pub struct Poll {
    pub paused: bool,
    pub redraw: bool,
}
impl Coordinator {
    pub const fn new() -> Self {
        Self {
            policy: Policy::new(),
            restore_haptics: true,
            last_epoch: 0,
        }
    }
    fn publish(&self) {
        critical_section::with(|cs| STATE.borrow(cs).set(self.policy.state()));
    }

    async fn restore(&mut self) -> bool {
        update_restore(WakeCycle::restoring);
        self.publish();
        let restored = crate::services::display::plane::set_sleep(false).await;
        self.policy.restored(restored);
        if restored {
            crate::services::feedback::lights::set_suspended(false);
            crate::services::feedback::haptic::set_enabled(self.restore_haptics);
        }
        self.publish();
        log::info!("power: restore={} (fresh full frame required)", restored);
        restored
    }

    /// Called before the UI handles OTA or renders. Admission uses current UI
    /// state, not a cached permission from the console or a timer callback.
    pub async fn poll(&mut self, blocker: Option<&'static str>) -> Poll {
        let mut redraw = false;
        let epoch = WAKE_EPOCH.load(Ordering::Acquire);
        if matches!(self.policy.state(), State::Entering | State::ScreenOff)
            && (crate::services::power::button_task::shutdown_requested()
                || crate::services::update::ota::device::active()
                || !crate::console::transport::connected())
        {
            self.policy.wake();
        }
        if self.policy.state() == State::Fault && epoch != self.last_epoch {
            self.policy.wake();
        }
        self.last_epoch = epoch;
        self.policy.poll(Instant::now().as_millis(), epoch);
        if self.policy.state() == State::Restoring {
            redraw |= self.restore().await;
        }

        if let Some(request) = PENDING.try_take() {
            if ACTIVE_REQUEST.load(Ordering::Acquire) == request.id {
                let result = match request.command {
                    Command::Status => Ok(()),
                    Command::ScreenOn => {
                        self.policy.wake();
                        if self.policy.state() == State::Active {
                            Ok(())
                        } else if self.restore().await {
                            redraw = true;
                            Ok(())
                        } else {
                            Err("panel restore failed")
                        }
                    }
                    Command::ScreenOff(seconds) => {
                        let blocker = blocker
                            .or_else(|| {
                                crate::services::update::ota::device::blocks_standby()
                                    .then_some("OTA or boot health window")
                            })
                            .or_else(|| {
                                crate::services::power::button_task::shutdown_requested()
                                    .then_some("shutdown pending")
                            })
                            .or_else(|| {
                                (!crate::console::transport::connected())
                                    .then_some("console disconnected")
                            });
                        if let Some(error) = blocker {
                            Err(error)
                        } else if request.wake_epoch != WAKE_EPOCH.load(Ordering::Acquire) {
                            Err("input or work arrived")
                        } else if let Err(error) = self.policy.request(
                            seconds,
                            Instant::now().as_millis(),
                            request.wake_epoch,
                        ) {
                            Err(error)
                        } else {
                            update_restore(WakeCycle::begin);
                            self.publish();
                            self.restore_haptics = crate::services::feedback::haptic::enabled();
                            crate::services::feedback::lights::set_suspended(true);
                            let quiet = if crate::services::feedback::haptic::available() {
                                matches!(
                                    select(
                                        crate::services::feedback::haptic::quiesce(),
                                        Timer::after_millis(100)
                                    )
                                    .await,
                                    Either::First(())
                                )
                            } else {
                                crate::services::feedback::haptic::set_enabled(false);
                                true
                            };
                            let entered =
                                quiet && crate::services::display::plane::set_sleep(true).await;
                            if ACTIVE_REQUEST.load(Ordering::Acquire) != request.id {
                                wake();
                            }
                            self.policy.entered(
                                entered,
                                Instant::now().as_millis(),
                                WAKE_EPOCH.load(Ordering::Acquire),
                            );
                            if crate::services::power::button_task::shutdown_requested()
                                || !crate::console::transport::connected()
                            {
                                self.policy.wake();
                            }
                            self.publish();
                            if self.policy.state() == State::Restoring {
                                redraw |= self.restore().await;
                                Err(if entered {
                                    "standby interrupted; wake requested"
                                } else {
                                    "peripheral standby failed"
                                })
                            } else {
                                log::info!("power: ScreenOff; CPU running, bounded manual lease");
                                Ok(())
                            }
                        }
                    }
                };
                DONE.signal((request.id, result));
            }
        }
        self.publish();
        Poll {
            paused: self.policy.state() != State::Active,
            redraw,
        }
    }
}
