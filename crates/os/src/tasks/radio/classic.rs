//! BR/EDR audio service. A native worker owns blocking Bluedroid lifecycle calls;
//! this task keeps pumping HCI until every host thread has acknowledged stop.
use alloc::{collections::VecDeque, format, string::String, vec::Vec};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering},
};
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::Timer;
use os::radio::BluetoothState;
use os::radio::lifecycle::{Gate, Pause};
static UI_GATE: Gate = Gate::new();

/// Called by the UI between callbacks, before standby or OTA can suspend it.
pub fn ui_checkpoint() -> bool {
    UI_GATE.checkpoint()
}

async fn pause_ui() -> Result<Pause<'static>, &'static str> {
    let pause = UI_GATE.request().ok_or("UI pause busy")?;
    embassy_time::with_timeout(embassy_time::Duration::from_secs(2), async {
        while !pause.acknowledged() {
            Timer::after_millis(1).await;
        }
    })
    .await
    .map_err(|_| "UI pause timeout")?;
    Ok(pause)
}
static READY: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
static STATE: AtomicU8 = AtomicU8::new(0);
static CONNECTIONS: AtomicU32 = AtomicU32::new(0);
static RX: AtomicU32 = AtomicU32::new(0);
static TX: AtomicU32 = AtomicU32::new(0);
static AUTH: AtomicU32 = AtomicU32::new(0);
static TRANSPORT_ERROR: AtomicBool = AtomicBool::new(false);
static REQUEST: Channel<CriticalSectionRawMutex, bool, 1> = Channel::new();
static DONE: Signal<CriticalSectionRawMutex, String> = Signal::new();
static HCI: Mutex<CriticalSectionRawMutex, RefCell<VecDeque<Vec<u8>>>> =
    Mutex::new(RefCell::new(VecDeque::new()));
static OP: AtomicU8 = AtomicU8::new(0);
static HOST_DONE: AtomicBool = AtomicBool::new(false);
static HOST_RESULT: AtomicI32 = AtomicI32::new(0);
static VOLUME_PENDING: AtomicBool = AtomicBool::new(false);
pub fn volume_changed() {
    VOLUME_PENDING.store(true, Ordering::Release);
}
pub fn state() -> BluetoothState {
    match STATE.load(Ordering::Acquire) {
        0 => BluetoothState::Off,
        1 => BluetoothState::Starting,
        2 => BluetoothState::Discoverable,
        3 => BluetoothState::Connected,
        4 => BluetoothState::Streaming,
        _ => BluetoothState::Fault,
    }
}
pub fn status() -> String {
    // Static profile/name information is returned by bt-on. Keep this record
    // short enough for full counters and the caller's internal_free field.
    format!("@ok bt-status state={} connections={} hci_rx={} hci_tx={} auth={} phase={}",state().name(),CONNECTIONS.load(Ordering::Relaxed),RX.load(Ordering::Relaxed),TX.load(Ordering::Relaxed),AUTH.load(Ordering::Relaxed),crate::drivers::radio::classic::current_phase())
}
pub async fn execute(on: bool) -> String {
    if !READY.load(Ordering::Acquire) {
        return "@err bt service starting".into();
    }
    if on
        && matches!(
            state(),
            BluetoothState::Discoverable | BluetoothState::Connected | BluetoothState::Streaming
        )
    {
        return "@ok bt-on already active".into();
    }
    if !on && state() == BluetoothState::Off {
        return "@ok bt-off state=off".into();
    }
    if on && super::super::audio::local_busy() {
        return "@err bt local audio busy".into();
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return "@err bt busy".into();
    }
    struct Pending;
    impl Drop for Pending {
        fn drop(&mut self) {
            PENDING.store(false, Ordering::Release);
        }
    }
    let _pending = Pending;
    DONE.reset();
    REQUEST.send(on).await;
    DONE.wait().await
}
// Blocking lifecycle calls run on the preemptible services thread. HCI keeps
// running on the priority-1 Core0 executor while these calls sleep/wait.
#[embassy_executor::task]
pub async fn host_task() {
    loop {
        let op = OP.swap(0, Ordering::AcqRel);
        if op != 0 {
            let result = unsafe {
                if op == 1 {
                    esp_bluedroid::mosaico_classic_volume(u32::from(
                        super::super::audio::snapshot().volume,
                    ));
                    esp_bluedroid::mosaico_classic_start()
                } else {
                    esp_bluedroid::mosaico_classic_stop()
                }
            };
            HOST_RESULT.store(result, Ordering::Release);
            HOST_DONE.store(true, Ordering::Release);
        }
        if VOLUME_PENDING.swap(false, Ordering::AcqRel) && state().enabled() {
            unsafe {
                esp_bluedroid::mosaico_classic_volume(u32::from(
                    super::super::audio::snapshot().volume,
                ));
            }
        }
        Timer::after_millis(1).await;
    }
}
fn host_op(op: u8) {
    HOST_DONE.store(false, Ordering::Release);
    OP.store(op, Ordering::Release);
}
#[embassy_executor::task]
pub async fn task(mut device: esp_hal::peripherals::BT<'static>) {
    if !super::super::battery::wait_boot().await { return; }
    crate::drivers::radio::classic::init_adapters();
    READY.store(true, Ordering::Release);
    loop {
        if !REQUEST.receive().await {
            STATE.store(0, Ordering::Release);
            DONE.signal("@ok bt-off state=off".into());
            continue;
        }
        if super::super::buttons::shutdown_requested() {
            DONE.signal("@err bt shutdown pending".into());
            continue;
        }
        if !crate::drivers::storage::ready() {
            DONE.signal("@err bt storage starting".into());
            continue;
        }
        let mut lifecycle = match pause_ui().await {
            Ok(pause) => Some(pause),
            Err(error) => {
                DONE.signal(format!("@err bt {error}"));
                continue;
            }
        };
        if super::super::buttons::shutdown_requested() {
            DONE.signal("@err bt shutdown pending".into());
            continue;
        }
        STATE.store(1, Ordering::Release);
        crate::drivers::radio::classic::phase(1);
        HCI.lock(|q| q.borrow_mut().clear());
        TRANSPORT_ERROR.store(false, Ordering::Release);
        let mut controller = match crate::drivers::radio::classic::controller(device.reborrow()) {
            Ok(c) => c,
            Err(e) => {
                STATE.store(5, Ordering::Release);
                DONE.signal(format!("@err bt {e}"));
                continue;
            }
        };
        log::info!("classic: controller enabled; starting Bluedroid A2DP sink");
        crate::drivers::radio::classic::phase(2);
        host_op(1);
        let mut starting = true;
        let mut stopping = false;
        let mut fault = None;
        let mut buffer = alloc::vec![0; 1030];
        // #region debug-point G-I:hci-service-gaps
        #[cfg(feature = "radio-hang-diagnostics")]
        let mut pump_trace = (embassy_time::Instant::now(), 0u64, embassy_time::Instant::now(), 0u32);
        // #endregion
        loop {
            // #region debug-point G-I:hci-service-gaps
            #[cfg(feature = "radio-hang-diagnostics")]
            {
                let now = embassy_time::Instant::now();
                pump_trace.1 = pump_trace.1.max(now.duration_since(pump_trace.0).as_micros());
                pump_trace.0 = now;
                pump_trace.3 += 1;
                if now.duration_since(pump_trace.2).as_millis() >= 1000 {
                    log::info!("classic pump: loops={} max_gap_us={}",pump_trace.3,pump_trace.1);
                    pump_trace.1 = 0;
                    pump_trace.2 = now;
                    pump_trace.3 = 0;
                }
            }
            // #endregion
            for _ in 0..16 {
                let packet = HCI.lock(|q| q.borrow_mut().pop_front());
                let Some(packet) = packet else {
                    break;
                };
                match controller.write(&packet) {
                    Ok(n) if n == packet.len() => {
                        TX.fetch_add(1, Ordering::Relaxed);
                        // #region debug-point J-L:a2dp-coexistence-command
                        #[cfg(feature = "radio-hang-diagnostics")]
                        if packet.len() == 7 && packet[..4] == [1, 0x82, 0xfc, 3] {
                            // Espressif common COEX_STATUS (0xfc82), whose
                            // payload is status type, set/clear, status mask.
                            log::info!("classic coex: tx type={} op={} mask={:#x}", packet[4], packet[5], packet[6]);
                        }
                        // #endregion
                    }
                    _ => {
                        fault = Some("HCI transmit");
                    }
                }
                unsafe {
                    esp_bluedroid::mosaico_classic_send_available();
                }
            }
            for _ in 0..32 {
                match controller.next(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        RX.fetch_add(1, Ordering::Relaxed);
                        // #region debug-point J-L:a2dp-coexistence-command
                        #[cfg(feature = "radio-hang-diagnostics")]
                        if n >= 7 && buffer[..2] == [4, 0x0e] && buffer[4..6] == [0x82, 0xfc] {
                            log::info!("classic coex: complete status={}", buffer[6]);
                        }
                        // #endregion
                        if unsafe {
                            esp_bluedroid::mosaico_classic_receive(buffer.as_mut_ptr(), n as u16)
                        } != 0
                        {
                            fault = Some("HCI receive rejected");
                        }
                    }
                    Err(_) => {
                        fault = Some("HCI receive");
                        break;
                    }
                }
            }
            if HOST_DONE.swap(false, Ordering::AcqRel) {
                let result = HOST_RESULT.load(Ordering::Acquire);
                if starting {
                    starting = false;
                    if result == 0 {
                        // #region debug-point D-E:classic-audio-routing-task-stacks
                        #[cfg(feature = "radio-hang-diagnostics")]
                        unsafe {
                            unsafe extern "Rust" { fn _rtos_trace_task_list(); }
                            _rtos_trace_task_list();
                        }
                        // #endregion
                        STATE.store(2, Ordering::Release);
                        drop(lifecycle.take());
                        DONE.signal(
                            "@ok bt-on name=ESP-Mosaico_Audio profile=a2dp-sink codec=sbc".into(),
                        );
                    } else {
                        STATE.store(5, Ordering::Release);
                        DONE.signal(format!("@err bt host start code={result}"));
                        if unsafe { esp_bluedroid::mosaico_classic_host_uninitialized() } {
                            break;
                        }
                        drop(lifecycle.take());
                    }
                } else if stopping {
                    if result == 0 && unsafe { esp_bluedroid::mosaico_classic_host_uninitialized() }
                    {
                        let audio = super::super::audio::quiesce().await;
                        STATE.store(if audio { 0 } else { 5 }, Ordering::Release);
                        DONE.signal(if audio {
                            format!("@ok bt-off state=off fault={}", fault.unwrap_or("none"))
                        } else {
                            "@err bt codec stop failed".into()
                        });
                        break;
                    }
                    stopping = false;
                    drop(lifecycle.take());
                    STATE.store(5, Ordering::Release);
                    DONE.signal(format!(
                        "@err bt host stop code={result}; controller retained"
                    ));
                }
            }
            if TRANSPORT_ERROR.swap(false, Ordering::AcqRel) {
                fault = Some("HCI queue overflow");
            }
            if !starting && !stopping {
                if let Ok(on) = REQUEST.try_receive() {
                    if on {
                        DONE.signal("@err bt host requires off before retry".into());
                    } else {
                        match pause_ui().await {
                            Ok(pause) => lifecycle = Some(pause),
                            Err(error) => {
                                DONE.signal(format!("@err bt {error}; controller retained"));
                                continue;
                            }
                        }
                        super::super::audio::bluetooth_active(false);
                        host_op(2);
                        stopping = true;
                    }
                } else if fault.is_some() {
                    match pause_ui().await {
                        Ok(pause) => lifecycle = Some(pause),
                        Err(_) => {
                            Timer::after_millis(10).await;
                            continue;
                        }
                    }
                    super::super::audio::bluetooth_active(false);
                    host_op(2);
                    stopping = true;
                }
            }
            Timer::after_millis(1).await;
        }
        // Controller teardown is reached only after host deinit succeeded.
        drop(controller);
        drop(lifecycle);
        HCI.lock(|q| q.borrow_mut().clear());
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_hci_send(data: *mut u8, len: u16) {
    let bytes = core::slice::from_raw_parts(data, len as usize);
    HCI.lock(|q| {
        let mut q = q.borrow_mut();
        if q.len() >= 32 {
            TRANSPORT_ERROR.store(true, Ordering::Release);
        } else {
            q.push_back(bytes.to_vec());
        }
    });
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_hci_available() -> bool {
    HCI.lock(|q| q.borrow().len() < 32)
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_event(kind: u32, a: u32, b: u32) {
    match kind {
        1 => match a {
            0 => {
                STATE.store(2, Ordering::Release);
                super::super::audio::bluetooth_active(false);
            }
            2 => {
                STATE.store(3, Ordering::Release);
                CONNECTIONS.fetch_add(1, Ordering::Relaxed);
            }
            _ => (),
        },
        2 => {
            let active = a == 1;
            STATE.store(if active { 4 } else { 3 }, Ordering::Release);
            super::super::audio::bluetooth_active(active);
        }
        3 => super::super::audio::bluetooth_format(a, b as u8),
        4 => {
            AUTH.store(a, Ordering::Release);
            log::info!("classic: pairing result={a}");
        }
        _ => (),
    }
    log::info!("classic: event={kind} a={a} b={b}");
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_pcm(data: *const u8, len: u32) {
    super::super::audio::bluetooth_pcm(core::slice::from_raw_parts(data, len as usize));
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_remote_volume(v: u32) {
    super::super::audio::remote_volume(v as u8);
}
