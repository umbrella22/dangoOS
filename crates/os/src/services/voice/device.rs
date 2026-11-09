//! Session owner. Networking never owns codec/LVGL and late updates are tagged.
use super::{config, network, storage, Command, Phase, Snapshot};
use crate::services::{audio, power, update};
use alloc::{boxed::Box, format, string::String};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_net::Stack;
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex},
    signal::Signal,
};
use embassy_time::{with_timeout, Duration, Timer};

static INFO: Mutex<CriticalSectionRawMutex, RefCell<Snapshot>> =
    Mutex::new(RefCell::new(Snapshot {
        session: 0,
        phase: Phase::Idle,
        recorded_ms: 0,
        transcript: String::new(),
        reply: String::new(),
        reply_start: 0,
        reply_bytes: 0,
        reply_path: String::new(),
        error: None,
    }));
static REVISION: AtomicU32 = AtomicU32::new(1);
static BUSY: AtomicBool = AtomicBool::new(false);
static FINISH: AtomicBool = AtomicBool::new(false);
static CONFIGURED: AtomicBool = AtomicBool::new(false);
// BUSY admits one turn until cleanup completes, so no accepted request can
// replace an unread request in this single-consumer signal.
static REQUEST: Signal<CriticalSectionRawMutex, (u32, Option<String>)> = Signal::new();
static CANCEL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static PAGE: Signal<CriticalSectionRawMutex, (u32, usize)> = Signal::new();

pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}
pub fn revision() -> u32 {
    REVISION.load(Ordering::Acquire)
}
pub fn snapshot() -> Snapshot {
    INFO.lock(|s| s.borrow().clone())
}
fn current_phase() -> Phase {
    INFO.lock(|s| s.borrow().phase)
}
fn change(id: u32, apply: impl FnOnce(&mut Snapshot) -> bool) {
    let changed = INFO.lock(|s| {
        let mut s = s.borrow_mut();
        s.session == id && apply(&mut s)
    });
    if changed {
        REVISION.fetch_add(1, Ordering::Release);
    }
}
pub(super) fn phase(id: u32, phase: Phase) {
    change(id, |s| {
        if s.phase == phase {
            false
        } else {
            s.phase = phase;
            true
        }
    });
}
pub(super) fn recorded(id: u32, ms: u32) {
    change(id, |s| {
        if s.recorded_ms / 200 == ms / 200 {
            false
        } else {
            s.recorded_ms = ms;
            true
        }
    });
}
pub(super) fn transcript(id: u32, text: &str) {
    publish_text(id, text, false);
}
pub(super) fn archive(id: u32, path: String) {
    change(id, |s| {
        s.reply_path = path;
        true
    });
}
pub(super) fn stored(id: u32, total: usize, window: storage::Window) {
    change(id, |s| {
        s.reply_bytes = total;
        s.reply_start = window.start;
        s.reply = window.text;
        true
    });
}
pub fn page(offset: usize) {
    PAGE.signal((INFO.lock(|s| s.borrow().session), offset));
}
pub(super) fn record() -> (u32, String, usize) {
    INFO.lock(|s| {
        let s = s.borrow();
        (s.session, s.reply_path.clone(), s.reply_bytes)
    })
}
pub(super) fn stored_window(id: u32, window: storage::Window) {
    change(id, |s| {
        s.reply_start = window.start;
        s.reply = window.text;
        true
    });
}
async fn read_page(id: u32, offset: usize) {
    if let Err(e) = storage::device::run(storage::device::Job::Page(id, offset)).await {
        change(id, |s| {
            s.phase = Phase::Error;
            s.error = Some(e);
            true
        });
    }
}

#[inline(never)]
fn publish_text(id: u32, text: &str, is_reply: bool) {
    change(id, |s| {
        let target = if is_reply {
            &mut s.reply
        } else {
            &mut s.transcript
        };
        target.clear();
        target.push_str(text);
        true
    });
}
pub(super) fn finish_requested() -> bool {
    FINISH.load(Ordering::Acquire)
}
pub fn cancel() {
    if busy() {
        CANCEL.signal(());
    }
}

pub fn toggle() -> bool {
    if busy() {
        let finishing = INFO.lock(|s| {
            let mut s = s.borrow_mut();
            if s.phase == Phase::Listening {
                s.phase = Phase::Transcribing;
                Some(s.session)
            } else {
                None
            }
        });
        if let Some(id) = finishing {
            FINISH.store(true, Ordering::Release);
            // Stop the producer even while DNS/TLS/WebSocket is connecting.
            // The network consumer still drains this session's complete tail.
            audio::capture::finish(id);
            REVISION.fetch_add(1, Ordering::Release);
        } else {
            cancel();
        }
        return true;
    }
    start(None)
}

fn start(text: Option<String>) -> bool {
    if busy() {
        return false;
    }
    if power::button_task::shutdown_requested()
        || update::boot::device::blocks_standby()
        || update::task::busy()
        || crate::apps::device::is_managing()
    {
        INFO.lock(|s| {
            let mut s = s.borrow_mut();
            s.phase = Phase::Error;
            s.error = Some("设备正在更新、初始化或管理应用，请稍后重试");
        });
        REVISION.fetch_add(1, Ordering::Release);
        return false;
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return false;
    }
    FINISH.store(false, Ordering::Release);
    CANCEL.reset();
    let id = INFO.lock(|s| {
        let mut s = s.borrow_mut();
        s.session = s.session.wrapping_add(1).max(1);
        s.phase = Phase::Preparing;
        s.recorded_ms = 0;
        s.transcript.clear();
        s.reply.clear();
        s.reply_start = 0;
        s.reply_bytes = 0;
        s.reply_path.clear();
        s.error = None;
        s.session
    });
    REVISION.fetch_add(1, Ordering::Release);
    REQUEST.signal((id, text));
    power::coordinator::wake();
    true
}
pub async fn quiesce() -> bool {
    cancel();
    with_timeout(Duration::from_secs(3), async {
        while busy() {
            Timer::after_millis(10).await;
        }
    })
    .await
    .is_ok()
}
pub fn status() -> String {
    let internal = crate::platform::runtime::heap::snapshot(app_host::caps::INTERNAL);
    let external = crate::platform::runtime::heap::snapshot(app_host::caps::SPIRAM);
    INFO.lock(|s| {
        let s = s.borrow();
        format!(
            concat!(
                "@voice-heap capability=internal free={} largest={} largest_valid={}\n",
                "@voice-heap capability=psram free={} largest={} largest_valid={}\n",
                "@voice-error {}\n",
                "@voice-record path={} offset={} window={} total={}\n",
                "@ok voice-status configured={} busy={} session={} state={} ",
                "recorded_ms={} transcript_bytes={} reply_bytes={} font_ready={} error={}"
            ),
            internal.free_size,
            internal.largest_free_block,
            u8::from(internal.valid_fields & app_host::heap_fields::LARGEST_FREE != 0),
            external.free_size,
            external.largest_free_block,
            u8::from(external.valid_fields & app_host::heap_fields::LARGEST_FREE != 0),
            s.error.unwrap_or("none"),
            s.reply_path,
            s.reply_start,
            s.reply.len(),
            s.reply_bytes,
            u8::from(CONFIGURED.load(Ordering::Acquire)),
            u8::from(busy()),
            s.session,
            s.phase.name(),
            s.recorded_ms,
            s.transcript.len(),
            s.reply_bytes,
            u8::from(crate::services::resources::voice_font::active().is_some()),
            u8::from(s.error.is_some())
        )
    })
}

fn load_config() -> Result<config::Config, &'static str> {
    fs::global::with(|v| Ok(config::load(v)))
        .map_err(|_| "语音配置存储不可用")
        .and_then(|v| v)
}

pub async fn execute(command: Command) -> String {
    match command {
        command @ (Command::Status | Command::Reload) => {
            let reload = command == Command::Reload;
            if reload && busy() {
                return "@err voice session active".into();
            }
            if !busy() {
                let result = crate::platform::runtime::executors::services::run(load_config).await;
                CONFIGURED.store(result.is_ok(), Ordering::Release);
                if reload {
                    return match result {
                        Ok(_) => "@ok voice-reload configured=1".into(),
                        Err(e) => format!("@err voice-reload {e}"),
                    };
                }
            }
            status()
        }
        Command::Desktop(command) => super::desktop::device::execute(command).await,
        Command::Toggle => {
            if toggle() {
                "@ok voice-toggle accepted".into()
            } else {
                "@err voice busy or unavailable".into()
            }
        }
        Command::Cancel => {
            cancel();
            "@ok voice-cancel requested".into()
        }
        Command::Ask(text) => {
            if start(Some(text)) {
                "@ok voice-ask accepted".into()
            } else {
                "@err voice busy or unavailable".into()
            }
        }
        Command::Read(offset) => {
            if busy() {
                "@err voice session active".into()
            } else {
                match storage::device::run(storage::device::Job::Read(offset)).await {
                    Ok(storage::device::Output::Text(line)) => line,
                    Err(e) => format!("@err {e}"),
                    _ => unreachable!(),
                }
            }
        }
        Command::Page(offset) => {
            if busy() {
                return "@err voice session active".into();
            }
            page(offset);
            "@ok voice-page requested".into()
        }
    }
}
async fn run_turn(
    stack: Stack<'static>,
    id: u32,
    text: Option<String>,
    desktop: Option<u32>,
) -> Result<(), &'static str> {
    if let (Some(nonce), Some(text)) = (desktop, text.as_ref()) {
        crate::services::resources::voice_font::prepare().await?;
        transcript(id, text);
        phase(id, Phase::Replying);
        return super::desktop::device::answer(nonce, id, text.clone()).await;
    }
    let config = crate::platform::runtime::executors::services::run(load_config).await;
    CONFIGURED.store(config.is_ok(), Ordering::Release);
    let config = config?;
    if !stack.is_config_up() {
        return Err("请先连接 Wi-Fi");
    }
    crate::services::resources::voice_font::prepare().await?;
    crate::platform::drivers::radio::https::sync_clock(stack)
        .await
        .map_err(|_| "无法同步时间以验证语音服务证书")?;
    let text = if let Some(text) = text {
        transcript(id, &text);
        text
    } else {
        audio::capture::initialize().await?;
        audio::task::start_voice(id)?;
        with_timeout(Duration::from_secs(5), async {
            loop {
                let status = audio::capture::status(id)?;
                if let Some(e) = status.error {
                    return Err(e);
                }
                if status.ready {
                    return Ok(());
                }
                Timer::after_millis(5).await;
            }
        })
        .await
        .map_err(|_| "麦克风启动超时")??;
        phase(id, Phase::Listening);
        network::transcribe(stack, &config, id).await?
    };
    phase(id, Phase::Replying);
    if let Some(nonce) = desktop {
        super::desktop::device::answer(nonce, id, text).await
    } else {
        network::answer(stack, &config, id, &text).await
    }
}
async fn watch(stack: Stack<'static>, id: u32, desktop: bool) -> &'static str {
    loop {
        if power::button_task::shutdown_requested() {
            return "设备正在关机";
        }
        if update::boot::device::active() || update::task::busy() {
            return "更新操作中止了语音会话";
        }
        if let Ok(s) = audio::capture::status(id) {
            recorded(id, s.ms);
            if let Some(error) = s.error {
                return error;
            }
        }
        if matches!(
            current_phase(),
            Phase::Listening | Phase::Transcribing | Phase::Replying
        ) && !(desktop && current_phase() == Phase::Replying)
            && !stack.is_config_up()
        {
            return "Wi-Fi 已断开，语音会话已停止";
        }
        Timer::after_millis(20).await;
    }
}
#[embassy_executor::task]
pub async fn task(stack: Stack<'static>) {
    loop {
        let (id, text) = match select(REQUEST.wait(), PAGE.wait()).await {
            Either::First(request) => request,
            Either::Second((id, offset)) => {
                read_page(id, offset).await;
                continue;
            }
        };
        // Move protocol/TLS futures out of permanent Internal task storage.
        // The general allocator prefers PSRAM; DMA keeps explicit capability.
        let desktop = super::desktop::device::attached();
        let outcome = select3(
            Box::pin(run_turn(stack, id, text, desktop)),
            CANCEL.wait(),
            watch(stack, id, desktop.is_some()),
        )
        .await;
        let (state, error) = match outcome {
            Either3::First(Ok(())) => (Phase::Done, None),
            Either3::First(Err(e)) => (Phase::Error, Some(e)),
            Either3::Second(()) => (Phase::Cancelled, None),
            Either3::Third(e) => (Phase::Error, Some(e)),
        };
        audio::capture::abort(id);
        let stopped = with_timeout(Duration::from_secs(3), async {
            while audio::capture::status(id).is_ok_and(|s| !s.closed) {
                Timer::after_millis(5).await;
            }
        })
        .await
        .is_ok();
        let saved = storage::device::run(storage::device::Job::Finish(
            id,
            if stopped { state } else { Phase::Error },
        ))
        .await;
        change(id, |s| {
            if let Err(e) = saved {
                s.phase = Phase::Error;
                s.error = Some(e);
                return true;
            }
            s.phase = if stopped { state } else { Phase::Error };
            s.error = if stopped {
                error
            } else {
                Some("麦克风停止超时")
            };
            true
        });
        BUSY.store(false, Ordering::Release);
        log::info!(
            "voice: session={id} state={}",
            if stopped { state.name() } else { "error" }
        );
    }
}
