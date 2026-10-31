//! GPIO7/61 low-active buttons and GPIO57 open-drain power latch.
//! Owns all three pins; independent of I2C/slot probing and Lua lifetime.
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use critical_section::Mutex;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Instant, Timer};
use esp_hal::gpio::{Input, Output};
use log::{info, warn};
use lua_engine::OnboardButtons;
use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::SensorEvent,
};
use {
    crate::services::power::buttons::Button, crate::services::power::buttons::Gesture,
    crate::services::power::buttons::Shutdown, crate::services::power::buttons::SLEEP_VISIBLE_MS,
};

static SNAPSHOT: Mutex<RefCell<OnboardButtons>> = Mutex::new(RefCell::new(OnboardButtons {
    ready: 0,
    pressed: [0; 2],
    sequence: 0,
    edges: [lua_engine::ButtonEdge {
        sequence: 0,
        at_ms: 0,
        key: 0,
        pressed: 0,
    }; 16],
}));
static POWER: Mutex<RefCell<Shutdown>> = Mutex::new(RefCell::new(Shutdown::Idle));
static LOW_BATTERY: Mutex<RefCell<Option<crate::services::power::battery::Shutdown>>> =
    Mutex::new(RefCell::new(None));

static STOP_RADIO: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static RADIO_QUIET: AtomicBool = AtomicBool::new(false);

// Controller operations can take seconds or fail. Keep them off the task that
// owns GPIO57, without cancelling/reusing their untagged request/reply channels.
#[embassy_executor::task]
pub async fn shutdown_radio_task() {
    STOP_RADIO.wait().await;
    while !crate::services::radio::device::quiesce().await {
        Timer::after_millis(100).await;
    }
    RADIO_QUIET.store(true, Ordering::Release);
}

pub fn snapshot() -> OnboardButtons {
    critical_section::with(|cs| *SNAPSHOT.borrow_ref(cs))
}
pub fn shutdown_requested() -> bool {
    low_battery_requested() || critical_section::with(|cs| POWER.borrow_ref(cs).requested())
}
pub fn low_battery_requested() -> bool {
    critical_section::with(|cs| LOW_BATTERY.borrow_ref(cs).is_some())
}
pub fn request_low_battery() {
    let first = critical_section::with(|cs| {
        let mut low = LOW_BATTERY.borrow_ref_mut(cs);
        if low.is_some() {
            return false;
        }
        *low = Some(crate::services::power::battery::Shutdown::new(
            Instant::now().as_millis(),
        ));
        true
    });
    if first {
        crate::services::update::ota::device::orderly_shutdown();
        crate::services::power::coordinator::wake();
        crate::services::feedback::lights::begin_shutdown();
        crate::services::feedback::haptic::set_enabled(false);
        crate::services::camera::task::set_ui_preview(false);
        crate::services::audio::task::request_stop();
        crate::services::update::task::cancel_for_shutdown();
        info!("power: low battery latched; warning then physical shutdown, no software reset");
    }
}
pub fn warning_uploaded(completed: bool) {
    critical_section::with(|cs| {
        if let Some(low) = LOW_BATTERY.borrow_ref_mut(cs).as_mut() {
            low.uploaded(Instant::now().as_millis(), completed);
        }
    });
}
pub fn frame_uploaded(sleep_frame: bool, completed: bool) {
    critical_section::with(|cs| {
        POWER
            .borrow_ref_mut(cs)
            .uploaded(Instant::now().as_millis(), sleep_frame, completed)
    });
}

#[embassy_executor::task]
pub async fn button_task(ai: Input<'static>, boot: Input<'static>, mut power: Output<'static>) {
    let now = Instant::now().as_millis();
    let mut keys = [
        Button::new(now, ai.is_low()),
        Button::new(now, boot.is_low()),
    ];
    critical_section::with(|cs| {
        let mut s = SNAPSHOT.borrow_ref_mut(cs);
        s.ready = 1;
        s.pressed = [keys[0].pressed() as u32, keys[1].pressed() as u32];
    });
    info!("buttons: AI GPIO7 / BOOT GPIO61 low-active, 20ms debounce, 180ms clicks, 1500ms long");
    let mut missing_frame_warned = false;
    let mut missing_clear_warned = false;
    let mut radio_stop_requested = false;
    let mut normal_cut_pending = false;
    loop {
        let now = Instant::now().as_millis();
        if low_battery_requested() {
            if !radio_stop_requested {
                radio_stop_requested = true;
                STOP_RADIO.signal(());
            }
            let cut = critical_section::with(|cs| {
                LOW_BATTERY
                    .borrow_ref_mut(cs)
                    .as_mut()
                    .is_some_and(|low| low.poll(Instant::now().as_millis()))
            });
            if cut {
                info!("power: low battery warning/deadline complete; asserting GPIO57 low");
                power.set_low();
            }
            // USB can retain supply after the latch is asserted. Stay on the
            // terminal warning; no new buttons/navigation/reboot are admitted.
            Timer::after_millis(5).await;
            continue;
        }
        for (index, pressed) in [ai.is_low(), boot.is_low()].into_iter().enumerate() {
            let update = keys[index].sample(now, pressed);
            if let Some(pressed) = update.edge {
                crate::services::power::coordinator::wake();
                critical_section::with(|cs| {
                    SNAPSHOT.borrow_ref_mut(cs).edge(index, pressed, now as u32)
                });
            }
            if let Some(gesture) = update.gesture {
                let source = if index == 0 {
                    EventSource::ButtonPower
                } else {
                    EventSource::ButtonBoot
                };
                let kind = match gesture {
                    Gesture::Short => EventKind::ButtonShort,
                    Gesture::Double => EventKind::ButtonDouble,
                    Gesture::Long => EventKind::ButtonLong,
                };
                info!("button {source:?} {gesture:?}");
                if index == 0 && gesture == Gesture::Long {
                    let first = critical_section::with(|cs| POWER.borrow_ref_mut(cs).request(now));
                    if first {
                        crate::services::update::ota::device::orderly_shutdown();
                        crate::services::feedback::lights::begin_shutdown();
                        info!("power: shutdown latched; waiting for presented Sleep frame");
                    }
                }
                crate::services::input::sensors::publish(SensorEvent {
                    at_ms: now,
                    source,
                    kind,
                });
            }
        }
        let lights_cleared = crate::services::feedback::lights::shutdown_cleared();
        let (cut, waiting, clear_waiting) = critical_section::with(|cs| {
            let mut p = POWER.borrow_ref_mut(cs);
            let cut = p.poll(now, lights_cleared);
            let waiting = matches!(*p,Shutdown::WaitingForSleep {requested_at} if now.saturating_sub(requested_at)>=5000);
            let clear_waiting = !lights_cleared
                && matches!(*p,
                Shutdown::WaitingForSleep {requested_at} if now.saturating_sub(requested_at)>=5000)
                || !lights_cleared
                    && matches!(*p,
                    Shutdown::ShowingSleep {since} if now.saturating_sub(since)>=SLEEP_VISIBLE_MS);
            (cut, waiting, clear_waiting)
        });
        if waiting && !missing_frame_warned {
            missing_frame_warned = true;
            warn!("power: no presented Sleep frame; keeping GPIO57 released");
        }
        if clear_waiting && !missing_clear_warned {
            missing_clear_warned = true;
            warn!("power: no shutdown black TX/latch receipt; keeping GPIO57 released");
        }
        normal_cut_pending |= cut;
        if normal_cut_pending && !radio_stop_requested {
            radio_stop_requested = true;
            STOP_RADIO.signal(());
        }
        if normal_cut_pending && RADIO_QUIET.load(Ordering::Acquire) {
            info!("power: Sleep >=1500ms and lights cleared; asserting GPIO57 open-drain low once");
            power.set_low();
            normal_cut_pending = false;
        }
        Timer::after_millis(5).await;
    }
}
