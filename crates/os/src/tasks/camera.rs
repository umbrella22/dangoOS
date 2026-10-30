//! Serializes camera requests; frame consumers never touch hardware registers.
use crate::drivers::camera::{
    Controls,
    dvp::{Dvp, HEIGHT, WIDTH},
    slot,
};
use alloc::{format, string::String, sync::Arc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Instant, Timer};
use os::camera::{Command, Snapshot, State, image};

static STATE: critical_section::Mutex<Cell<Snapshot>> =
    critical_section::Mutex::new(Cell::new(Snapshot {
        state: State::Off,
        pid: 0,
        frames: 0,
        dropped: 0,
        width: 0,
        height: 0,
        saved: 0,
        error: None,
    }));
static FRAME: critical_section::Mutex<RefCell<Option<Arc<Preview>>>> =
    critical_section::Mutex::new(RefCell::new(None));
static NEXT: AtomicU32 = AtomicU32::new(1);
static REQUESTS: Channel<CriticalSectionRawMutex, (u32, Command), 4> = Channel::new();
static REPLIES: Channel<CriticalSectionRawMutex, (u32, Result<(), &'static str>), 1> =
    Channel::new();
static SERIAL: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
static UI_PREVIEW: AtomicBool = AtomicBool::new(false);
static UI_REV: AtomicU32 = AtomicU32::new(0);
static PREFERENCES: AtomicU32 = AtomicU32::new(85 << 1);
pub fn set_preferences(settings: os::hub::Settings) {
    PREFERENCES.store(
        (u32::from(settings.jpeg_quality) << 1) | u32::from(settings.camera_mirror),
        Ordering::Release,
    );
}
pub struct Preview {
    pub sequence: u32,
    pub pixels: Vec<u16>,
}
pub fn frame() -> Option<Arc<Preview>> {
    critical_section::with(|cs| FRAME.borrow_ref(cs).clone())
}
pub fn snapshot() -> Snapshot {
    critical_section::with(|cs| STATE.borrow(cs).get())
}
fn publish(state: Snapshot) {
    critical_section::with(|cs| STATE.borrow(cs).set(state));
}
pub fn busy() -> bool {
    matches!(snapshot().state, State::Starting | State::Preview)
}
pub fn set_ui_preview(enabled: bool) {
    UI_PREVIEW.store(enabled, Ordering::Release);
    UI_REV.fetch_add(1, Ordering::AcqRel);
}
pub fn request_capture() -> bool {
    REQUESTS.try_send((0, Command::Capture)).is_ok()
}
pub fn status() -> String {
    let s = snapshot();
    format!(
        "@ok camera-status state={} sensor={} pid={:04x} size={}x{} frames={} drop={} saved={} error={}",
        s.state.name(),
        s.sensor(),
        s.pid,
        s.width,
        s.height,
        s.frames,
        s.dropped,
        s.saved,
        s.error.unwrap_or("none")
    )
}
pub async fn execute(command: Command) -> String {
    if command == Command::Status {
        return status();
    }
    let _guard = SERIAL.lock().await;
    if command != Command::Stop && (crate::ota_runtime::blocks_standby() || super::buttons::shutdown_requested()) {
        return format!("@err {} OTA busy", command.verb());
    }
    if !super::power::ensure_awake().await {
        return format!("@err {} display unavailable", command.verb());
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
    REQUESTS.send((id, command)).await;
    let (reply_id, result) = REPLIES.receive().await;
    assert_eq!(reply_id, id);
    match result {
        Ok(()) => format!(
            "@ok {} {} pid={:04x}",
            command.verb(),
            snapshot().sensor(),
            snapshot().pid
        ),
        Err(error) => format!("@err {} {error}", command.verb()),
    }
}
/// OTA waits for the camera task to stop DMA and return the left-slot lease.
pub async fn quiesce() -> bool {
    execute(Command::Stop).await.starts_with("@ok")
}

fn stop(
    controls: &mut Controls,
    dvp: &mut Dvp,
    active: &mut Option<slot::Lease>,
    s: &mut Snapshot,
) {
    dvp.stop();
    controls.off();
    if let Some(mut lease) = active.take() {
        Controls::release_reset(&mut lease);
        slot::release(lease);
    }
    s.state = State::Off;
    publish(*s);
    critical_section::with(|cs| FRAME.borrow_ref_mut(cs).take());
}
async fn start(
    controls: &mut Controls,
    dvp: &mut Dvp,
    active: &mut Option<slot::Lease>,
    s: &mut Snapshot,
    probe: bool,
) -> Result<(), &'static str> {
    if active.is_some() {
        return if probe {
            Err("camera-streaming")
        } else {
            Ok(())
        };
    }
    if crate::ota_runtime::blocks_standby() {
        return Err("OTA or boot health window");
    }
    if super::buttons::shutdown_requested() {
        return Err("shutdown pending");
    }
    s.state = State::Starting;
    s.error = None;
    publish(*s);
    let mut lease = slot::acquire().await?;
    let result = async {
        s.pid = controls.identify(&mut lease).await?;
        if probe {
            return Ok(());
        }
        controls.configure().await?;
        dvp.start(lease.data.take().ok_or("camera-slot-data")?)?;
        // Discard partial startup frames; a successful command requires a full frame.
        let mut last = Err("camera-no-frame");
        for _ in 0..4 {
            last = dvp.receive().await;
            if last.is_ok() {
                break;
            }
        }
        last?;
        s.width = WIDTH as u16;
        s.height = HEIGHT as u16;
        Ok(())
    }
    .await;
    *active = Some(lease);
    if probe || result.is_err() {
        stop(controls, dvp, active, s);
    } else {
        s.state = State::Preview;
        publish(*s);
    }
    result
}
async fn preview(dvp: &Dvp, s: &mut Snapshot, recycle: &mut Option<Arc<Preview>>) {
    let mut next = recycle.take().unwrap_or_else(|| {
        Arc::new(Preview {
            sequence: 0,
            pixels: alloc::vec![0;image::PREVIEW_WIDTH*image::PREVIEW_HEIGHT],
        })
    });
    if Arc::get_mut(&mut next).is_none() {
        *recycle = Some(next);
        return;
    }
    let out = Arc::get_mut(&mut next).unwrap();
    out.sequence = s.frames;
    let mirror = PREFERENCES.load(Ordering::Acquire) & 1 != 0;
    for y in 0..image::PREVIEW_HEIGHT {
        image::preview_row(
            dvp.pixels(),
            WIDTH,
            HEIGHT,
            y,
            &mut out.pixels[y * image::PREVIEW_WIDTH..(y + 1) * image::PREVIEW_WIDTH],
            mirror,
        );
        if y % 16 == 15 {
            embassy_futures::yield_now().await;
        }
    }
    *recycle = critical_section::with(|cs| FRAME.borrow_ref_mut(cs).replace(next));
}
fn save_photo(dvp: &Dvp) -> Result<String, &'static str> {
    let preferences = PREFERENCES.load(Ordering::Acquire);
    let rgb = image::rgb888(dvp.pixels(), WIDTH, HEIGHT, preferences & 1 != 0)?;
    let jpeg = lua_engine::jpeg::encode_rgb888(
        WIDTH as u16,
        HEIGHT as u16,
        &rgb,
        (preferences >> 1) as u8,
    )
    .map_err(|_| "photo-encode")?;
    ::fs::global::with(|vfs| {
        let root = "/data/appdata/photo/photos";
        vfs.create_dir_all(root)?;
        let mut index = 1u32;
        let path = loop {
            let p = format!("{root}/camera-{index:06}.jpg");
            match vfs.stat(&p) {
                Ok(_) => index = index.checked_add(1).ok_or(::fs::Errno::NOSPC)?,
                Err(::fs::Errno::NOENT) => break p,
                Err(e) => return Err(e),
            }
        };
        let temporary = format!("{root}/.camera-pending");
        vfs.write_file(&temporary, &jpeg)?;
        vfs.rename(&temporary, &path)?;
        Ok(path)
    })
    .map_err(|_| "photo-storage")
}
#[embassy_executor::task]
pub async fn task(hardware: Hardware) {
    // Construct all !Send async HAL drivers on their owning executor/Core0.
    let Hardware(power, flash, cam, dma, pclk, vsync, href, d0, d1, d2, d3) = hardware;
    let mut controls = Controls::new(power, flash);
    let mut dvp = Dvp::new(cam, dma, pclk, vsync, href, d0, d1, d2, d3);
    let mut active = None;
    let mut s = snapshot();
    let mut ui_rev = 0;
    let mut recycle = None;
    let mut errors = 0;
    let mut next = Instant::now();
    loop {
        if active.is_some()
            && (!slot::present()
                || super::buttons::shutdown_requested()
                || crate::ota_runtime::active())
        {
            stop(&mut controls, &mut dvp, &mut active, &mut s);
        }
        let rev = UI_REV.load(Ordering::Acquire);
        let request = if rev != ui_rev {
            ui_rev = rev;
            Some((
                0,
                if UI_PREVIEW.load(Ordering::Acquire) {
                    Command::Start
                } else {
                    Command::Stop
                },
            ))
        } else if active.is_some() {
            match select(REQUESTS.receive(), Timer::at(next)).await {
                Either::First(r) => Some(r),
                Either::Second(()) => None,
            }
        } else {
            match select(REQUESTS.receive(), Timer::after_millis(20)).await {
                Either::First(r) => Some(r),
                Either::Second(()) => continue,
            }
        };
        if let Some((id, command)) = request {
            let result = if command != Command::Stop && super::buttons::shutdown_requested() {
                Err("shutdown-pending")
            } else { match command {
                Command::Probe | Command::Start => {
                    start(
                        &mut controls,
                        &mut dvp,
                        &mut active,
                        &mut s,
                        command == Command::Probe,
                    )
                    .await
                }
                Command::Stop => {
                    stop(&mut controls, &mut dvp, &mut active, &mut s);
                    Ok(())
                }
                Command::Capture => {
                    if active.is_none() {
                        Err("camera-not-started")
                    } else {
                        match dvp.receive().await.and_then(|_| save_photo(&dvp)) {
                            Ok(path) => {
                                s.saved += 1;
                                log::info!("camera: saved {path}");
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                }
                Command::Status => Ok(()),
            }};
            if let Err(error) = result {
                s.error = Some(error);
                if active.is_none() {
                    s.state = State::Fault;
                }
            }
            publish(s);
            if id != 0 {
                REPLIES.send((id, result)).await;
            }
            next = Instant::now();
        } else {
            match dvp.receive().await {
                Ok(_) => {
                    errors = 0;
                    s.frames += 1;
                    s.error = None;
                    preview(&dvp, &mut s, &mut recycle).await;
                }
                Err(error) => {
                    errors += 1;
                    s.dropped += 1;
                    s.error = Some(error);
                    if errors >= 3 {
                        stop(&mut controls, &mut dvp, &mut active, &mut s);
                        s.state = State::Fault;
                    }
                }
            }
            publish(s);
            next = Instant::now() + embassy_time::Duration::from_millis(50);
        }
    }
}

use esp_hal::peripherals::*;
/// Unconfigured peripheral ownership is Send; initialized async drivers stay
/// on the services executor that creates them.
pub struct Hardware(pub GPIO48<'static>, pub GPIO34<'static>, pub LCD_CAM<'static>, pub DMA_AXI_CH2<'static>,
    pub GPIO16<'static>, pub GPIO15<'static>, pub GPIO33<'static>, pub GPIO18<'static>,
    pub GPIO55<'static>, pub GPIO19<'static>, pub GPIO17<'static>);
