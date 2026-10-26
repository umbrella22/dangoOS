//! GPIO7/61 low-active buttons and GPIO57 open-drain power latch.
//! Owns all three pins; independent of I2C/slot probing and Lua lifetime.
use core::cell::RefCell;
use critical_section::Mutex;
use embassy_time::{Instant, Timer};
use esp_hal::gpio::{Input, Output};
use log::{info, warn};
use lua_engine::OnboardButtons;
use os::behavior::affect::{EventKind, EventSource, SensorEvent};
use os::buttons::{Button, Gesture, Shutdown, SLEEP_VISIBLE_MS};

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

pub fn snapshot() -> OnboardButtons {
    critical_section::with(|cs| *SNAPSHOT.borrow_ref(cs))
}
pub fn shutdown_requested() -> bool {
    critical_section::with(|cs| POWER.borrow_ref(cs).requested())
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
    loop {
        let now = Instant::now().as_millis();
        for (index, pressed) in [ai.is_low(), boot.is_low()].into_iter().enumerate() {
            let update = keys[index].sample(now, pressed);
            if let Some(pressed) = update.edge {
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
                        super::lights::begin_shutdown();
                        info!("power: shutdown latched; waiting for presented Sleep frame");
                    }
                }
                super::sensors::publish(SensorEvent {
                    at_ms: now,
                    source,
                    kind,
                });
            }
        }
        let lights_cleared = super::lights::shutdown_cleared();
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
        if cut {
            info!("power: Sleep >=1500ms and lights cleared; asserting GPIO57 open-drain low once");
            power.set_low();
        }
        Timer::after_millis(5).await;
    }
}
