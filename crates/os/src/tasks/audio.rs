//! One owner for codec and I2S. File IO runs on the caller's thread; the DMA
//! worker runs on an interrupt executor and never enters VFS or flash writes.
use crate::drivers::audio::{
    codec::Codec,
    pcm::{Io, Stream},
};
use alloc::{format, string::String, vec::Vec};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex},
    channel::Channel,
    signal::Signal,
};
use embassy_time::{Instant, Timer};
use os::audio::{
    bluetooth_prefill_frames, loopback, wav, Command, Format, PcmQueue, Snapshot,
    State, BLUETOOTH_QUEUE_FRAMES,
};
static INFO: Mutex<CriticalSectionRawMutex, RefCell<Snapshot>> =
    Mutex::new(RefCell::new(Snapshot::new()));
static VOLUME: AtomicU8 = AtomicU8::new(50);
static MUTED: AtomicBool = AtomicBool::new(false);
static CHANGED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
pub fn request_stop() { STOP.store(true, Ordering::Release); }
static STOPPED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
struct Bluetooth {
    epoch: u32,
    format: Option<Format>,
    active: bool,
    queue: Option<PcmQueue>,
    drops: u32,
    // #region debug-point N-O-P:pcm-delivery
    #[cfg(feature = "radio-hang-diagnostics")]
    delivery: DeliveryTrace,
    // #endregion
}
// #region debug-point N-O-P:pcm-delivery
#[cfg(feature = "radio-hang-diagnostics")]
#[derive(Clone, Copy)]
struct DeliveryTrace {
    calls: u32,
    last_us: u32,
    max_gap_us: u32,
    max_packet_frames: u32,
    dropped_frames: u32,
    queue_peak: usize,
    silence_frames: u32,
    gaps: [u32; 8],
}
#[cfg(feature = "radio-hang-diagnostics")]
impl DeliveryTrace {
    const fn new() -> Self {
        Self {
            calls: 0,
            last_us: 0,
            max_gap_us: 0,
            max_packet_frames: 0,
            dropped_frames: 0,
            queue_peak: 0,
            silence_frames: 0,
            gaps: [0; 8],
        }
    }
}
// #endregion
static BT: Mutex<CriticalSectionRawMutex, RefCell<Bluetooth>> =
    Mutex::new(RefCell::new(Bluetooth {
        epoch: 0,
        format: None,
        active: false,
        queue: None,
        drops: 0,
        #[cfg(feature = "radio-hang-diagnostics")]
        delivery: DeliveryTrace::new(),
    }));
struct Request {
    format: Format,
    source: Source,
}
enum Source {
    Tone {
        hz: u16,
        frames: u32,
        phase: u32,
    },
    Wave {
        data: Vec<u8>,
        offset: usize,
        end: usize,
    },
    Record {
        data: Vec<u8>,
        frames: u32,
        probe_frame: Option<u32>,
    },
}
static REQUEST: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
static DONE: Signal<CriticalSectionRawMutex, Result<Vec<u8>, &'static str>> = Signal::new();
pub fn snapshot() -> Snapshot {
    INFO.lock(|s| *s.borrow())
}
fn update(f: impl FnOnce(&mut Snapshot)) {
    INFO.lock(|s| f(&mut s.borrow_mut()));
}
pub fn local_busy() -> bool {
    PENDING.load(Ordering::Acquire)
}
pub fn volume(value: u8) {
    set_volume(value, true);
}
pub fn remote_volume(value: u8) {
    set_volume(value, false);
}
fn set_volume(value: u8, notify_remote: bool) {
    if value <= 100 {
        VOLUME.store(value, Ordering::Release);
        CHANGED.store(true, Ordering::Release);
        update(|s| s.volume = value);
        if notify_remote {
            super::radio::classic::volume_changed();
        }
    }
}
pub fn mute(value: bool) {
    MUTED.store(value, Ordering::Release);
    CHANGED.store(true, Ordering::Release);
    update(|s| s.muted = value);
}
pub fn bluetooth_format(rate: u32, channels: u8) {
    BT.lock(|v| {
        let mut v = v.borrow_mut();
        v.epoch = v.epoch.wrapping_add(1);
        v.format = Format::new(rate, channels).ok();
        if let Some(q) = &mut v.queue {
            q.clear();
        }
    });
}
pub fn bluetooth_active(on: bool) {
    BT.lock(|v| {
        let mut v = v.borrow_mut();
        // #region debug-point N-O-P:pcm-delivery
        #[cfg(feature = "radio-hang-diagnostics")]
        {
            if on && !v.active {
                v.delivery = DeliveryTrace::new();
            }
            v.delivery.last_us = 0;
        }
        // #endregion
        v.epoch = v.epoch.wrapping_add(1);
        v.active = on;
        if let Some(q) = &mut v.queue {
            q.clear();
        }
    });
}
pub fn bluetooth_pcm(bytes: &[u8]) {
    BT.lock(|v| {
        let mut v = v.borrow_mut();
        if !v.active {
            return;
        }
        let Some(f) = v.format else {
            return;
        };
        let rejected = v.queue
            .as_mut()
            .is_some_and(|q| q.push(bytes, f.channels).is_err());
        if rejected {
            v.drops = v.drops.saturating_add(1);
        }
        // #region debug-point N-O-P:pcm-delivery
        #[cfg(feature = "radio-hang-diagnostics")]
        {
            let now = Instant::now().as_micros() as u32;
            let queued = v.queue.as_ref().map_or(0, PcmQueue::len);
            let frames = bytes.len() as u32 / (u32::from(f.channels) * 2);
            let trace = &mut v.delivery;
            trace.calls = trace.calls.saturating_add(1);
            if trace.last_us != 0 {
                let gap = now.wrapping_sub(trace.last_us);
                trace.max_gap_us = trace.max_gap_us.max(gap);
                let bucket = [2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000]
                    .iter()
                    .position(|&limit| gap <= limit)
                    .unwrap_or(7);
                trace.gaps[bucket] = trace.gaps[bucket].saturating_add(1);
            }
            trace.last_us = now;
            trace.max_packet_frames = trace.max_packet_frames.max(frames);
            trace.queue_peak = trace.queue_peak.max(queued);
            if rejected {
                trace.dropped_frames = trace.dropped_frames.saturating_add(frames);
            }
        }
        // #endregion
    });
}
pub fn status() -> String {
    let s = snapshot();
    let result = format!("@ok audio-status state={} rate={} volume={} muted={} tx_frames={} rx_frames={} underruns={} overruns={} peak={} error={}",s.state.name(),s.rate,s.volume,u8::from(s.muted),s.tx_frames,s.rx_frames,s.underruns,s.overruns,s.peak,s.error.unwrap_or("none"));
    // #region debug-point N-O-P:pcm-delivery
    #[cfg(feature = "radio-hang-diagnostics")]
    {
        let trace = BT.lock(|b| b.borrow().delivery);
        // The console has 158 text bytes per record. Keep observations on
        // separate protocol lines before the final acknowledgement so the
        // host receives every field without mistaking an observation for ACK.
        return format!(
            "@audio-pcm calls={} gap_us={} packet_frames={} drop_frames={}\n\
             @audio-buffer queue_peak={} silence_frames={} pump_gap_us={}\n\
             @audio-gaps {:?}\n{result}",
            trace.calls, trace.max_gap_us, trace.max_packet_frames,
            trace.dropped_frames, trace.queue_peak, trace.silence_frames,
            MAX_PUMP_GAP_US.load(Ordering::Relaxed), trace.gaps,
        );
    }
    // #endregion
    #[cfg(not(feature = "radio-hang-diagnostics"))]
    result
}
pub async fn quiesce() -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    bluetooth_active(false);
    STOPPED.reset();
    STOP.store(true, Ordering::Release);
    embassy_time::with_timeout(embassy_time::Duration::from_secs(3), STOPPED.wait())
        .await
        .is_ok()
        && snapshot().state == State::Off
}
pub async fn execute(command: Command) -> String {
    match command {
        Command::Status => return status(),
        Command::Volume(v) => {
            volume(v);
            return format!("@ok audio-volume value={v}");
        }
        Command::Mute(v) => {
            mute(v);
            return format!("@ok audio-mute value={}", u8::from(v));
        }
        Command::Stop => {
            return if quiesce().await {
                "@ok audio-stop state=off".into()
            } else {
                "@err audio stop failed".into()
            }
        }
        _ => (),
    }
    if !READY.load(Ordering::Acquire) {
        return "@err audio service starting".into();
    }
    if crate::ota_runtime::active() || super::update::busy() || super::buttons::shutdown_requested() {
        return "@err audio OTA busy".into();
    }
    if super::radio::classic::state().enabled() {
        return "@err audio disable Bluetooth before local playback or recording".into();
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return "@err audio busy".into();
    }
    struct Pending;
    impl Drop for Pending {
        fn drop(&mut self) {
            PENDING.store(false, Ordering::Release);
        }
    }
    let _pending = Pending;
    let verb = command.verb();
    let mut save = None;
    let request = match command {
        Command::Tone { hz, ms } => Request {
            format: Format::new(48_000, 2).unwrap(),
            source: Source::Tone {
                hz,
                frames: 48 * u32::from(ms),
                phase: 0,
            },
        },
        Command::Play(path) => {
            let data = match crate::service_executor::run(move || fs::global::with(|v| {
                let stat = v.stat(&path)?;
                if stat.size > 8 * 1024 * 1024 {
                    return Err(fs::Errno::FBIG);
                }
                v.read_file(&path)
            })).await {
                Ok(d) => d,
                Err(_) => return "@err audio WAV read failed or larger than 8 MiB".into(),
            };
            let wave = match wav::parse(&data) {
                Ok(w) => w,
                Err(e) => return format!("@err audio {e}"),
            };
            Request {
                format: wave.format,
                source: Source::Wave {
                    data,
                    offset: wave.offset,
                    end: wave.offset + wave.bytes,
                },
            }
        }
        Command::Record { path, ms } => {
            save = Some(path);
            let frames = 16 * u32::from(ms);
            Request {
                format: Format::SPEECH,
                source: Source::Record {
                    data: Vec::with_capacity(frames as usize * 2 + 44),
                    frames,
                    probe_frame: None,
                },
            }
        }
        Command::Loopback(path) => {
            save = Some(path);
            Request {
                format: Format::SPEECH,
                source: Source::Record {
                    data: Vec::with_capacity(loopback::FRAMES as usize * 2 + 44),
                    frames: loopback::FRAMES,
                    probe_frame: Some(0),
                },
            }
        }
        _ => unreachable!(),
    };
    if super::buttons::shutdown_requested() { return "@err audio shutdown pending".into(); }
    STOP.store(false, Ordering::Release);
    DONE.reset();
    REQUEST.send(request).await;
    match DONE.wait().await {
        Ok(data) => {
            if let Some(path) = save {
                let bytes = data.len();
                let owned_path = path.clone();
                let result = crate::service_executor::run(move || fs::global::with(|v| {
                    let path = owned_path;
                    let parent = path.rsplit_once('/').unwrap().0;
                    v.create_dir_all(parent)?;
                    let tmp = format!("{path}.tmp");
                    v.write_file(&tmp, &data)?;
                    v.rename(&tmp, &path)
                })).await;
                if result.is_err() {
                    return "@err audio recording save failed".into();
                }
                format!("@ok {verb} bytes={bytes} path={path}")
            } else {
                format!("@ok {verb} complete")
            }
        }
        Err(e) => format!("@err audio {e}"),
    }
}
async fn stop(
    codec: &mut Codec,
    io: &mut Io,
    stream: &mut Option<Stream>,
) -> Result<(), &'static str> {
    codec.amplifier(false);
    if let Some(s) = stream.take() {
        io.stop(s);
    }
    codec.stop().await
}
async fn start(
    codec: &mut Codec,
    io: &mut Io,
    rate: u32,
    output: bool,
) -> Result<Stream, &'static str> {
    update(|s| {
        s.state = State::Starting;
        s.rate = rate;
        s.error = None;
    });
    let mut stream = io.start(rate)?;
    // ES8311 initialization needs a running MCLK. Its shared I2C transactions
    // can outlast the DMA ring, especially at 44.1/48 kHz. Keep both directions
    // serviced while configuration waits; otherwise the finite silent priming
    // ring can stop before the first decoded frame reaches it.
    let started = Instant::now();
    let mut primed = 0usize;
    let mut discard = [0u8; 1024];
    let result = {
        let configure = async {
            codec.start(rate, VOLUME.load(Ordering::Acquire), true).await?;
            codec
                .mute(MUTED.load(Ordering::Acquire) || VOLUME.load(Ordering::Acquire) == 0)
                .await
        };
        let mut configure = core::pin::pin!(configure);
        loop {
            match select(configure.as_mut(), Timer::after_millis(2)).await {
                Either::First(result) => break result,
                Either::Second(()) => {
                    let (_, silence) = stream.fill(|_| 0);
                    primed += silence;
                    let _ = stream.receive(&mut discard);
                }
            }
        }
    };
    log::info!("audio: codec start rate={rate} elapsed_us={} primed_frames={primed} ok={}", started.elapsed().as_micros(), result.is_ok());
    if let Err(e) = result {
        io.stop(stream);
        let _ = codec.stop().await;
        return Err(e);
    }
    codec.amplifier(output);
    Ok(stream)
}
// Service both DMA rings even while a codec-control future awaits shared I2C.
static LAST_PUMP_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static MAX_PUMP_GAP_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
fn pump_stream(s: &mut Stream, local: &mut Option<Request>, bt_mode: bool, scratch: &mut [u8]) -> (bool, bool) {
    let now = Instant::now().as_micros() as u32;
    let previous = LAST_PUMP_US.swap(now, Ordering::Relaxed);
    let gap = now.wrapping_sub(previous);
    if previous != 0 && gap > MAX_PUMP_GAP_US.load(Ordering::Relaxed) {
        MAX_PUMP_GAP_US.store(gap, Ordering::Relaxed);
        if gap >= 12_000 { log::warn!("audio: DMA pump max_gap_us={gap} bt={bt_mode}"); }
    }
    let mut exhausted = false;
    let (tx, silence) = s.fill(|out| {
        if bt_mode {
            return BT
                .lock(|b| b.borrow_mut().queue.as_mut().map_or(0, |q| q.pop_into(out)));
        }
        let Some(r) = local.as_mut() else {
            return 0;
        };
        match &mut r.source {
            Source::Tone { hz, frames, phase } => {
                let n = (out.len() / 4).min(*frames as usize);
                for slot in out[..n * 4].chunks_exact_mut(4) {
                    // Quiet triangle tone, no floating point or clipping.
                    *phase = phase.wrapping_add(u32::from(*hz));
                    if *phase >= r.format.rate {
                        *phase -= r.format.rate;
                    }
                    let p = *phase * 4 * 1200 / r.format.rate;
                    let v = if p < 2400 {
                        p as i32 - 1200
                    } else {
                        3600 - p as i32
                    } as i16;
                    slot[..2].copy_from_slice(&v.to_le_bytes());
                    slot[2..].copy_from_slice(&v.to_le_bytes());
                }
                *frames -= n as u32;
                exhausted = *frames == 0;
                n * 4
            }
            Source::Wave { data, offset, end } => {
                let frame = r.format.frame_bytes();
                let n = (out.len() / 4).min((*end - *offset) / frame);
                for slot in out[..n * 4].chunks_exact_mut(4) {
                    let l = i16::from_le_bytes([data[*offset], data[*offset + 1]]);
                    let rr = if frame == 4 {
                        i16::from_le_bytes([data[*offset + 2], data[*offset + 3]])
                    } else {
                        l
                    };
                    let mono = ((i32::from(l) + i32::from(rr)) / 2) as i16;
                    slot[..2].copy_from_slice(&mono.to_le_bytes());
                    slot[2..].copy_from_slice(&mono.to_le_bytes());
                    *offset += frame;
                }
                exhausted = *offset == *end;
                n * 4
            }
            Source::Record {
                probe_frame: Some(frame),
                ..
            } => {
                for slot in out.chunks_exact_mut(4) {
                    let value = loopback::sample(*frame).to_le_bytes();
                    slot[..2].copy_from_slice(&value);
                    slot[2..].copy_from_slice(&value);
                    *frame = frame.saturating_add(1);
                }
                out.len()
            }
            Source::Record {
                probe_frame: None, ..
            } => 0,
        }
    });
    let rx = s.receive(scratch);
    let mut peak = 0u16;
    if let Some(Request {
        source: Source::Record { data, frames, .. },
        ..
    }) = local.as_mut()
    {
        for frame in scratch[..rx].chunks_exact(4) {
            if *frames == 0 {
                break;
            }
            let sample = i16::from_le_bytes([frame[0], frame[1]]);
            peak = peak.max(sample.unsigned_abs());
            data.extend_from_slice(&frame[..2]);
            *frames -= 1;
        }
        exhausted = *frames == 0;
    }
    let drops = BT.lock(|b| {
        // #region debug-point N-O-P:pcm-delivery
        #[cfg(feature = "radio-hang-diagnostics")]
        if bt_mode {
            let mut b = b.borrow_mut();
            b.delivery.silence_frames = b.delivery.silence_frames.saturating_add(silence as u32);
        }
        // #endregion
        b.borrow().drops
    });
    update(|s| {
        s.tx_frames = s.tx_frames.saturating_add(tx as u32);
        s.rx_frames = s.rx_frames.saturating_add((rx / 4) as u32);
        s.overruns = drops;
        s.peak = peak;
        if bt_mode && silence != 0 {
            s.underruns = s.underruns.saturating_add(1);
        }
    });
    (exhausted, tx != 0 || silence != 0 || rx != 0)
}
#[embassy_executor::task]
pub async fn task(
    i2s: esp_hal::peripherals::I2S0<'static>,
    dma: esp_hal::peripherals::DMA_CH0<'static>,
    mclk: esp_hal::peripherals::GPIO54<'static>,
    bclk: esp_hal::peripherals::GPIO37<'static>,
    ws: esp_hal::peripherals::GPIO49<'static>,
    dout: esp_hal::peripherals::GPIO52<'static>,
    din: esp_hal::peripherals::GPIO40<'static>,
    pa: esp_hal::peripherals::GPIO45<'static>,
) {
    let mut io = Io::new(i2s, dma, mclk, bclk, ws, dout, din).expect("audio I2S initialization");
    let mut codec = Codec::new(pa);
    #[cfg(feature = "radio-hang-diagnostics")]
    let internal_before = esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into());
    BT.lock(|s| s.borrow_mut().queue = Some(PcmQueue::new(BLUETOOTH_QUEUE_FRAMES)));
    #[cfg(feature = "radio-hang-diagnostics")]
    log::info!("audio buffer: frames={} internal_before={} internal_after={}",
        BLUETOOTH_QUEUE_FRAMES, internal_before,
        esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into()));
    READY.store(true, Ordering::Release);
    let mut stream = None;
    let mut local: Option<Request> = None;
    let mut bt_epoch = 0;
    let mut bt_mode = false;
    let mut scratch = alloc::vec![0u8; 2048];
    let mut last_progress = Instant::now();
    let mut configured = false;
    loop {
        if super::buttons::low_battery_requested() {
            if stream.is_some() || configured {
                let _ = stop(&mut codec, &mut io, &mut stream).await;
                configured = false;
            }
            let cancelled = local.take().is_some();
            let queued = REQUEST.try_receive().is_ok();
            if cancelled || queued { DONE.signal(Err("low battery shutdown")); }
            bt_mode = false;
            update(|s| { s.state = State::Off; s.rate = 0; s.error = None; });
            STOPPED.signal(());
            Timer::after_millis(20).await;
            continue;
        }
        if STOP.swap(false, Ordering::AcqRel) {
            if local.take().is_some() {
                DONE.signal(Err("cancelled"));
            }
            let result = if stream.is_some() || configured {
                stop(&mut codec, &mut io, &mut stream).await
            } else {
                Ok(())
            };
            configured = false;
            bt_mode = false;
            update(|s| {
                s.state = if result.is_ok() {
                    State::Off
                } else {
                    State::Fault
                };
                s.rate = 0;
                s.error = result.err();
            });
            STOPPED.signal(());
        }
        if stream.is_none() {
            if let Ok(r) = REQUEST.try_receive() {
                local = Some(r);
            }
            let target = BT.lock(|s| {
                let s = s.borrow();
                (
                    s.epoch,
                    s.active,
                    s.format,
                    s.queue.as_ref().map_or(0, PcmQueue::len),
                )
            });
            let next = local.as_ref().map(|r| r.format.rate).or_else(|| {
                if target.1 {
                    target.2
                        .filter(|f| target.3 >= bluetooth_prefill_frames(*f))
                        .map(|f| f.rate)
                } else {
                    None
                }
            });
            if let Some(rate) = next {
                bt_mode = local.is_none();
                bt_epoch = target.0;
                let recording = local
                    .as_ref()
                    .is_some_and(|r| matches!(r.source, Source::Record { .. }));
                let output = !local.as_ref().is_some_and(|r| {
                    matches!(
                        r.source,
                        Source::Record {
                            probe_frame: None,
                            ..
                        }
                    )
                });
                match start(&mut codec, &mut io, rate, output).await {
                    Ok(mut s) => {
                        if recording {
                            // The ADC and RX ring have been running throughout
                            // codec setup. Discard startup/ramp samples before
                            // exposing a recording, while continuing to feed TX.
                            let settled = Instant::now() + embassy_time::Duration::from_millis(200);
                            while Instant::now() < settled {
                                s.fill(|_| 0);
                                let _ = s.receive(&mut scratch);
                                Timer::after_millis(2).await;
                            }
                        }
                        LAST_PUMP_US.store(0, Ordering::Relaxed);
                        MAX_PUMP_GAP_US.store(0, Ordering::Relaxed);
                        stream = Some(s);
                        configured = true;
                        last_progress = Instant::now();
                        update(|s| {
                            s.state = if recording {
                                State::Recording
                            } else {
                                State::Playing
                            }
                        });
                    }
                    Err(e) => {
                        codec.amplifier(false);
                        let _ = stop(&mut codec, &mut io, &mut stream).await;
                        if local.take().is_some() {
                            DONE.signal(Err(e));
                        }
                        if bt_mode {
                            bluetooth_active(false);
                        }
                        update(|s| {
                            s.state = State::Fault;
                            s.error = Some(e);
                        });
                    }
                }
            }
        }
        if let Some(s) = stream.as_mut() {
            let changed = BT.lock(|b| {
                let b = b.borrow();
                bt_mode && (!b.active || b.epoch != bt_epoch)
            });
            if changed {
                let r = stop(&mut codec, &mut io, &mut stream).await;
                configured = false;
                update(|s| {
                    s.state = if r.is_ok() { State::Off } else { State::Fault };
                    s.error = r.err();
                });
                continue;
            }
            let (exhausted, progress) = pump_stream(s, &mut local, bt_mode, &mut scratch);
            if progress { last_progress = Instant::now(); }
            let timeout = last_progress.elapsed().as_millis() > 250;
            if exhausted || timeout {
                if timeout { log::error!("audio: I2S progress timeout max_gap_us={}", MAX_PUMP_GAP_US.load(Ordering::Relaxed)); }
                // Drain the final TX descriptors before stopping the amplifier.
                if exhausted
                    && !matches!(
                        local.as_ref().map(|r| &r.source),
                        Some(Source::Record { .. })
                    )
                {
                    let periods = (2_048_000u32 / snapshot().rate.max(1) + 12) / 2;
                    for _ in 0..periods {
                        Timer::after_millis(2).await;
                        s.fill(|_| 0);
                        let _ = s.receive(&mut scratch);
                    }
                }
                let result = stop(&mut codec, &mut io, &mut stream).await;
                configured = false;
                let result = if timeout {
                    Err("I2S progress timeout")
                } else {
                    result
                };
                if let Some(r) = local.take() {
                    match result {
                        Err(e) => DONE.signal(Err(e)),
                        Ok(()) => {
                            let data = if let Source::Record { data, .. } = r.source {
                                let mut bytes =
                                    wav::header(r.format, data.len() as u32).unwrap().to_vec();
                                bytes.extend_from_slice(&data);
                                bytes
                            } else {
                                Vec::new()
                            };
                            DONE.signal(Ok(data));
                        }
                    }
                }
                if timeout && bt_mode {
                    bluetooth_active(false);
                }
                update(|s| {
                    s.state = if result.is_ok() {
                        State::Off
                    } else {
                        State::Fault
                    };
                    s.error = result.err();
                    s.rate = 0;
                });
            }
        }
        if CHANGED.swap(false, Ordering::AcqRel) && stream.is_some() {
            let v = VOLUME.load(Ordering::Acquire);
            let started = Instant::now();
            let r = {
                let configure = async {
                    codec.volume(v).await?;
                    codec.mute(MUTED.load(Ordering::Acquire) || v == 0).await
                };
                let mut configure = core::pin::pin!(configure);
                loop {
                    match select(configure.as_mut(), Timer::after_millis(2)).await {
                        Either::First(result) => break result,
                        Either::Second(()) => {
                            if let Some(s) = stream.as_mut() {
                                let (_, progress) = pump_stream(s, &mut local, bt_mode, &mut scratch);
                                if progress { last_progress = Instant::now(); }
                            }
                        }
                    }
                }
            };
            log::info!("audio: controls elapsed_us={} ok={}", started.elapsed().as_micros(), r.is_ok());
            if let Err(e) = r {
                codec.amplifier(false);
                STOP.store(true, Ordering::Release);
                update(|s| s.error = Some(e));
            }
        }
        Timer::after_millis(2).await;
    }
}
