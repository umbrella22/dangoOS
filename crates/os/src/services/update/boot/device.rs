//! The running main image confirms health and hands installation to Recovery.
use crate::platform::storage::nor::{self as storage, BootFlash};
#[cfg(feature = "lua-engine")]
use alloc::boxed::Box;
use alloc::{format, string::String};
use boot_contract::{Record, State};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use embassy_time::{Instant, Timer};

static HANDOFF: AtomicBool = AtomicBool::new(false);
static PAUSE_UI: AtomicBool = AtomicBool::new(false);
static OLD_HAPTIC: AtomicBool = AtomicBool::new(false);
static VM_OK: AtomicBool = AtomicBool::new(false);
static BOOT_STATE: AtomicU8 = AtomicU8::new(State::Rescue as u8);
static CONFIRMED: AtomicBool = AtomicBool::new(false);
static HEALTH: critical_section::Mutex<RefCell<super::HealthWindow>> =
    critical_section::Mutex::new(RefCell::new(super::HealthWindow::new()));

pub fn active() -> bool {
    HANDOFF.load(Ordering::Acquire)
}
pub fn pauses_ui() -> bool {
    PAUSE_UI.load(Ordering::Acquire)
}
pub fn candidate() -> bool {
    BOOT_STATE.load(Ordering::Acquire) == State::Testing as u8 && !CONFIRMED.load(Ordering::Acquire)
}
pub fn blocks_standby() -> bool {
    active() || candidate() || crate::services::update::task::busy()
}
pub fn booted_main() -> bool {
    let physical = critical_section::with(|_| unsafe {
        (0x20500380 as *mut u32).write_volatile(0);
        ((0x2050037c as *const u32).read_volatile() & 0x7ff) << 16
    });
    physical == boot_contract::MAIN_BASE
}
fn record() -> Result<Record, &'static str> {
    boot_contract::read(&mut BootFlash)
        .map_err(|_| "启动记录读取失败")?
        .map(|(_, record)| record)
        .ok_or("缺少启动记录")
}
pub fn early_boot() {
    if storage::verify_layout().is_err() || !booted_main() {
        log::error!("boot: main partition contract rejected");
        return;
    }
    match record() {
        Ok(r) if matches!(r.state, State::Ready | State::Testing) => {
            BOOT_STATE.store(r.state as u8, Ordering::Release);
            log::info!(
                "boot: layout=S31ONE03 state={:?} generation={}",
                r.state,
                r.generation
            );
        }
        _ => log::error!("boot: running main lacks an eligible boot record"),
    }
}
#[cfg(feature = "lua-engine")]
struct HealthHost;
#[cfg(feature = "lua-engine")]
impl lua_engine::Host for HealthHost {
    fn print(&mut self, _: &[u8]) {}
}
#[cfg(feature = "lua-engine")]
fn exercise_vm() -> bool {
    let Ok(mut vm) = lua_engine::Vm::new(
        Box::new(HealthHost),
        lua_engine::Limits {
            memory_bytes: 128 * 1024,
            ..Default::default()
        },
        &crate::apps::runtime::PSRAM,
        0x424f4f54,
    ) else {
        return false;
    };
    vm.set_callback_budget(20, 100_000);
    if vm
        .load(
            "boot-selftest",
            b"assert(2+2==4); assert(require('json').decode('{\"ok\":true}').ok)",
        )
        .is_err()
    {
        return false;
    }
    matches!(vm.resume(), lua_engine::Step::Done)
}
#[cfg(feature = "ts-engine")]
fn exercise_vm() -> bool {
    crate::apps::runtime::selftest()
}

pub fn health() -> String {
    let (system, data) = storage::mounted();
    format!(
        "sys={} data={} frame={} vm={} hb={} ready={}",
        system as u8,
        data as u8,
        crate::services::display::status::FIRST_FRAME.load(Ordering::Acquire) as u8,
        VM_OK.load(Ordering::Acquire) as u8,
        crate::shell::runtime::HEARTBEATS.load(Ordering::Acquire),
        CONFIRMED.load(Ordering::Acquire) as u8,
    )
}
pub fn status() -> String {
    let reply = match record() {
        Ok(r) => format!(
            "@ok boot-status layout=S31ONE03 state={:?} generation={} image_bytes={} {}",
            r.state,
            r.generation,
            r.image_bytes,
            health()
        ),
        Err(_) => format!("@err boot-status unavailable {}", health()),
    };
    let (inherited, splash_us, selected_us) = board_s31::boot_display::status();
    // Keep each line within the console's 158-byte text record, including the
    // existing status fields at their maximum widths. The terminal ACK stays
    // last so command clients receive both the diagnostic and the reply.
    format!(
        "@boot-display display_handoff={} splash_us={} selected_us={}\n{}",
        inherited as u8, splash_us, selected_us, reply,
    )
}
pub fn selftest(boot_ready: bool) {
    if CONFIRMED.load(Ordering::Acquire)
        || crate::services::power::button_task::shutdown_requested()
        || !matches!(BOOT_STATE.load(Ordering::Acquire), x if x == State::Ready as u8 || x == State::Testing as u8)
    {
        return;
    }
    let (system, data) = storage::mounted();
    let ready = boot_ready
        && !active()
        && storage::ready()
        && system
        && data
        && !crate::platform::runtime::crashlog::skip(
            crate::platform::runtime::crashlog::Stage::Storage,
        )
        && !crate::platform::runtime::crashlog::skip(
            crate::platform::runtime::crashlog::Stage::AppScan,
        );
    if ready && !VM_OK.load(Ordering::Acquire) {
        VM_OK.store(exercise_vm(), Ordering::Release);
    }
    let healthy = ready
        && VM_OK.load(Ordering::Acquire)
        && crate::services::display::status::FIRST_FRAME.load(Ordering::Acquire)
        && crate::shell::runtime::HEARTBEATS.load(Ordering::Acquire) >= 2;
    if !critical_section::with(|cs| {
        HEALTH
            .borrow_ref_mut(cs)
            .observe(Instant::now().as_millis(), healthy)
    }) {
        return;
    }
    if storage::verify_layout().is_err() || !booted_main() {
        return;
    }
    let Ok(r) = record() else {
        return;
    };
    let confirmed = match r.state {
        State::Ready => true,
        State::Testing => {
            boot_contract::append(&mut BootFlash, State::Ready, r.image_bytes, r.sha256).is_ok()
        }
        _ => false,
    };
    if confirmed {
        BOOT_STATE.store(State::Ready as u8, Ordering::Release);
        CONFIRMED.store(true, Ordering::Release);
        log::info!("boot: main health confirmed {}", health());
    }
}

async fn quiesce() -> Result<(), &'static str> {
    if crate::services::power::button_task::shutdown_requested()
        || !CONFIRMED.load(Ordering::Acquire)
        || crate::apps::device::is_managing()
        || HANDOFF
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err("系统尚未就绪或正在处理其他任务");
    }
    if !crate::services::voice::device::quiesce().await
        || !crate::services::audio::task::quiesce().await
        || !crate::services::camera::task::quiesce().await
        || !crate::services::radio::device::quiesce().await
        || !crate::services::power::coordinator::ensure_awake().await
    {
        HANDOFF.store(false, Ordering::Release);
        return Err("无法暂停设备服务");
    }
    PAUSE_UI.store(true, Ordering::Release);
    OLD_HAPTIC.store(
        crate::services::feedback::haptic::enabled(),
        Ordering::Release,
    );
    let result = embassy_futures::select::select(
        async {
            crate::services::feedback::haptic::quiesce().await;
            crate::apps::runtime::quiesce().await;
        },
        Timer::after_secs(5),
    )
    .await;
    if !matches!(result, embassy_futures::select::Either::First(())) {
        cancel_handoff();
        return Err("无法暂停应用");
    }
    if crate::services::power::button_task::shutdown_requested() {
        cancel_handoff();
        return Err("关机进行中");
    }
    Ok(())
}
pub fn cancel_handoff() {
    if !HANDOFF.swap(false, Ordering::AcqRel) {
        return;
    }
    PAUSE_UI.store(false, Ordering::Release);
    crate::apps::runtime::release_management();
    crate::services::feedback::haptic::set_enabled(OLD_HAPTIC.load(Ordering::Acquire));
}
pub async fn stage(bytes: u32, sha256: [u8; 32]) -> Result<(), &'static str> {
    storage::verify_layout().map_err(|_| "分区布局不匹配")?;
    quiesce().await?;
    if boot_contract::append(&mut BootFlash, State::Staged, bytes, sha256).is_err() {
        cancel_handoff();
        return Err("无法提交救援安装请求");
    }
    Ok(())
}
pub async fn enter_recovery() -> Result<(), &'static str> {
    storage::verify_layout().map_err(|_| "分区布局不匹配")?;
    quiesce().await?;
    if boot_contract::append(&mut BootFlash, State::Rescue, 0, [0; 32]).is_err() {
        cancel_handoff();
        return Err("无法提交救援启动请求");
    }
    Ok(())
}
