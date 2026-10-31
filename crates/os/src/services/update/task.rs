//! Download to NAND, close the network, then use the existing A/B installer.
use alloc::{format, string::String, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, Ordering},
};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Instant, Timer};
use sha2::{Digest, Sha256};
use {
    crate::services::update::Command, crate::services::update::HttpProxy,
    crate::services::update::Phase, crate::services::update::Release,
    crate::services::update::Snapshot, crate::services::update::Url,
};

static STATE: critical_section::Mutex<RefCell<Snapshot>> =
    critical_section::Mutex::new(RefCell::new(Snapshot {
        phase: Phase::Idle,
        version: None,
        received: 0,
        total: 0,
        error: None,
    }));
static BUSY: AtomicBool = AtomicBool::new(false);
static AUTO: AtomicBool = AtomicBool::new(true);
static PROXY: critical_section::Mutex<Cell<Option<HttpProxy>>> =
    critical_section::Mutex::new(Cell::new(None));
static REQUEST: Channel<CriticalSectionRawMutex, Command, 1> = Channel::new();
static CANCEL: Signal<CriticalSectionRawMutex, ()> = Signal::new();
const STAGE: &str = "/data/.update/firmware.bin";
const AUTO_INTERVAL: u64 = 6 * 60 * 60 * 1000;

pub fn snapshot() -> Snapshot {
    critical_section::with(|cs| STATE.borrow_ref(cs).clone())
}
fn publish(value: &Snapshot) {
    critical_section::with(|cs| *STATE.borrow_ref_mut(cs) = value.clone());
}
pub fn cancel_for_shutdown() {
    CANCEL.signal(());
}
pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}
pub fn configure(settings: crate::services::settings::preferences::Settings) {
    AUTO.store(settings.auto_update_check, Ordering::Release);
    let proxy = if settings.update_proxy_enabled {
        settings.update_proxy
    } else {
        None
    };
    critical_section::with(|cs| PROXY.borrow(cs).set(proxy));
}
pub fn status() -> String {
    let s = snapshot();
    format!(
        "@ok update-status phase={:?} version={} received={} total={} proxy={} error={}",
        s.phase,
        s.version.as_deref().unwrap_or("none"),
        s.received,
        s.total,
        critical_section::with(|cs| PROXY.borrow(cs).get())
            .map_or_else(|| "off".into(), HttpProxy::address),
        s.error.unwrap_or("none")
    )
}
pub fn submit(command: Command) -> bool {
    if crate::services::power::button_task::shutdown_requested() {
        return false;
    }
    if command == Command::Cancel {
        if matches!(
            snapshot().phase,
            Phase::Checking | Phase::Downloading | Phase::Verifying
        ) {
            CANCEL.signal(());
            return true;
        }
        return false;
    }
    if command == Command::Status {
        return false;
    }
    if let Command::SetProxy(proxy) = command {
        if busy()
            || crate::services::update::ota::device::active()
            || crate::services::update::ota::device::candidate()
        {
            return false;
        }
        crate::services::settings::device::configure_proxy(proxy);
        return true;
    }
    if crate::services::radio::device::wifi::busy()
        || crate::services::update::ota::device::active()
        || crate::services::update::ota::device::candidate()
        || crate::apps::device::is_managing()
        || BUSY.swap(true, Ordering::AcqRel)
    {
        return false;
    }
    if command == Command::Install && snapshot().phase != Phase::Available {
        BUSY.store(false, Ordering::Release);
        return false;
    }
    CANCEL.reset();
    if REQUEST.try_send(command).is_err() {
        BUSY.store(false, Ordering::Release);
        return false;
    }
    true
}

use crate::services::update::staging::File;
async fn close(file: File) -> Result<(), &'static str> {
    crate::platform::runtime::executors::services::run(move || {
        file.close_with(|fd| ::fs::global::with(|vfs| vfs.close(fd)))
    })
    .await
    .ok_or("更新文件正在使用")?
    .map_err(|_| "无法关闭更新文件")
}
async fn open(flags: ::fs::OpenFlags) -> Result<File, &'static str> {
    crate::platform::runtime::executors::services::run(move || {
        ::fs::global::with(|vfs| {
            vfs.create_dir_all("/data/.update")?;
            vfs.open(STAGE, flags).map(|fd| {
                File::new(
                    fd,
                    crate::platform::runtime::executors::services::close_later,
                )
            })
        })
    })
    .await
    .map_err(|_| "无法打开更新文件，请检查存储空间")
}
async fn manifest(
    stack: embassy_net::Stack<'_>,
    source: &Url,
    proxy: Option<HttpProxy>,
) -> Result<Release, &'static str> {
    crate::platform::drivers::radio::https::sync_clock(stack).await?;
    let mut bytes = Vec::new();
    crate::platform::drivers::radio::https::get(
        stack,
        source,
        proxy,
        crate::services::update::MANIFEST_BYTES as u32,
        None,
        async |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        },
    )
    .await?;
    Release::parse(&bytes, source)
}
async fn download(
    stack: embassy_net::Stack<'_>,
    release: &Release,
    proxy: Option<HttpProxy>,
    state: &mut Snapshot,
) -> Result<(), &'static str> {
    let file = open(::fs::OpenFlags::WRITE).await?;
    let mut sha = Sha256::new();
    let mut image = crate::services::update::ota::image::ImageVerifier::new(release.bytes);
    state.phase = Phase::Downloading;
    state.received = 0;
    state.total = release.bytes;
    publish(state);
    let mut last_percent = 0;
    crate::platform::drivers::radio::https::get(
        stack,
        &release.image,
        proxy,
        release.bytes,
        Some(release.bytes),
        async |bytes: &[u8]| {
            image.feed(bytes).map_err(|_| "固件镜像格式错误")?;
            let chunk = bytes.to_vec();
            let owned = file.clone();
            let n = crate::platform::runtime::executors::services::run(move || {
                ::fs::global::with(|vfs| vfs.write(owned.fd(), &chunk))
            })
            .await
            .map_err(|_| "更新文件写入失败")?;
            if n != bytes.len() {
                return Err("存储空间不足");
            }
            sha.update(bytes);
            state.received += bytes.len() as u32;
            let percent = u64::from(state.received) * 100 / u64::from(state.total);
            if percent != last_percent {
                last_percent = percent;
                publish(state);
            }
            Ok(())
        },
    )
    .await?;
    close(file).await?;
    state.phase = Phase::Verifying;
    publish(state);
    if sha.finalize().as_slice() != release.sha256 {
        return Err("固件校验失败，请重新下载");
    }
    image.finish().map_err(|_| "固件镜像格式错误")?;
    if !image.ab_layout() || image.version() != Some(release.version.as_str()) {
        return Err("固件版本或分区布局不匹配");
    }
    Ok(())
}
async fn install(release: &Release, state: &mut Snapshot) -> Result<(), &'static str> {
    state.phase = Phase::Installing;
    state.received = 0;
    publish(state);
    if !crate::services::camera::task::quiesce().await
        || !crate::services::radio::device::quiesce().await
    {
        return Err("无法暂停相机或无线连接");
    }
    if !crate::services::power::coordinator::ensure_awake().await {
        return Err("显示尚未就绪");
    }
    let file = open(::fs::OpenFlags::READ).await?;
    let mut updater = crate::services::update::ota::device::DeviceUpdater::new();
    let result = async {
        updater
            .execute(crate::services::update::ota::Command::Begin {
                size: release.bytes,
                sha: release.sha256,
            })
            .await
            .map_err(|_| "无法开始安装")?;
        let mut offset = 0;
        let mut last_percent = 0;
        while offset < release.bytes {
            let mut bytes = alloc::vec![0;4096.min((release.bytes-offset) as usize)];
            let owned = file.clone();
            let (next, result) = crate::platform::runtime::executors::services::run(move || {
                let result = ::fs::global::with(|vfs| vfs.read(owned.fd(), &mut bytes));
                (bytes, result)
            })
            .await;
            let bytes = next;
            let n = result.map_err(|_| "更新文件读取失败")?;
            if n != bytes.len() {
                return Err("更新文件不完整");
            }
            let crc = crate::formats::crc32::crc32(&bytes);
            updater
                .execute(crate::services::update::ota::Command::Chunk { offset, bytes, crc })
                .await
                .map_err(|_| "备用固件写入或回读失败")?;
            offset += n as u32;
            state.received = offset;
            let percent = u64::from(offset) * 100 / u64::from(state.total);
            if percent != last_percent {
                last_percent = percent;
                publish(state);
            }
        }
        close(file).await?;
        let _ = crate::platform::runtime::executors::services::run(|| {
            ::fs::global::with(|vfs| vfs.unlink(STAGE))
        })
        .await;
        updater
            .execute(crate::services::update::ota::Command::End)
            .await
            .map_err(|_| "固件回读校验失败")?;
        Ok::<(), &'static str>(())
    }
    .await;
    if result.is_err() {
        updater.abort();
    }
    result?;
    state.phase = Phase::Restarting;
    publish(state);
    log::info!("update: candidate installed, restarting");
    Timer::after_millis(800).await;
    if crate::services::power::button_task::shutdown_requested() {
        return Err("关机进行中");
    }
    esp_hal::system::software_reset();
}

#[embassy_executor::task]
pub async fn task(stack: embassy_net::Stack<'static>) {
    let source = Url::parse(
        option_env!("DANGO_UPDATE_MANIFEST_URL").unwrap_or(crate::services::update::MANIFEST_URL),
    )
    .expect("compiled update URL");
    let mut cached: Option<Release> = None;
    let mut last_auto: Option<u64> = None;
    loop {
        let command = match select(REQUEST.receive(), Timer::after_secs(2)).await {
            Either::First(command) => command,
            Either::Second(()) => {
                let now = Instant::now().as_millis();
                if crate::services::power::button_task::shutdown_requested()
                    || !AUTO.load(Ordering::Acquire)
                    || !crate::services::settings::device::preferences().loaded
                    || crate::services::radio::device::wifi::state()
                        != crate::services::radio::WifiState::Connected
                    || crate::services::radio::device::wifi::busy()
                    || crate::services::update::ota::device::active()
                    || crate::services::update::ota::device::candidate()
                    || crate::apps::device::is_managing()
                    || last_auto.is_some_and(|last| now.saturating_sub(last) < AUTO_INTERVAL)
                    || BUSY.swap(true, Ordering::AcqRel)
                {
                    continue;
                }
                CANCEL.reset();
                Command::Check
            }
        };
        let mut state = snapshot();
        let proxy = critical_section::with(|cs| PROXY.borrow(cs).get());
        state.error = None;
        let result = if crate::services::power::button_task::shutdown_requested() {
            Err("关机进行中")
        } else if crate::services::radio::device::wifi::state()
            != crate::services::radio::WifiState::Connected
        {
            Err("请先连接 Wi-Fi")
        } else {
            let operation = async {
                match command {
                    Command::Check => {
                        state.phase = Phase::Checking;
                        state.version = None;
                        state.received = 0;
                        state.total = 0;
                        publish(&state);
                        cached = None;
                        let release = manifest(stack, &source, proxy).await?;
                        state.phase = if release.newer_than(env!("CARGO_PKG_VERSION"))? {
                            Phase::Available
                        } else {
                            Phase::Current
                        };
                        state.version = Some(release.version.clone());
                        cached = Some(release);
                        publish(&state);
                        Ok(None)
                    }
                    Command::Install => {
                        let release = cached.clone().ok_or("请先检查更新")?;
                        download(stack, &release, proxy, &mut state).await?;
                        Ok(Some(release))
                    }
                    Command::InstallUrl(url) => {
                        state.phase = Phase::Checking;
                        publish(&state);
                        let release = manifest(stack, &url, proxy).await?;
                        state.version = Some(release.version.clone());
                        download(stack, &release, proxy, &mut state).await?;
                        Ok(Some(release))
                    }
                    _ => Err("更新操作无效"),
                }
            };
            match select(operation, CANCEL.wait()).await {
                Either::First(result) => match result {
                    Ok(Some(release)) => {
                        if CANCEL.try_take().is_some()
                            || crate::services::power::button_task::shutdown_requested()
                        {
                            state.phase = Phase::Cancelled;
                            Ok(())
                        } else {
                            install(&release, &mut state).await
                        }
                    }
                    Ok(None) => Ok(()),
                    Err(error) => Err(error),
                },
                Either::Second(()) => {
                    state.phase = Phase::Cancelled;
                    Ok(())
                }
            }
        };
        if let Err(error) = result {
            log::warn!("update: {error}");
            state.phase = if error == "release partition layout" {
                Phase::Incompatible
            } else {
                Phase::Failed
            };
            state.error = Some(if error == "release partition layout" {
                "发布固件使用旧分区布局，已拒绝安装"
            } else if error.starts_with("release") || error.starts_with("manifest") {
                "发布包信息不匹配，已停止更新"
            } else if error.is_ascii() {
                "更新源或网络连接异常，请稍后重试"
            } else {
                error
            });
        }
        if !matches!(state.phase, Phase::Available | Phase::Current) {
            let _ = crate::platform::runtime::executors::services::run(|| {
                ::fs::global::with(|vfs| vfs.unlink(STAGE))
            })
            .await;
        }
        publish(&state);
        last_auto = Some(Instant::now().as_millis());
        BUSY.store(false, Ordering::Release);
    }
}
