use crate::platform::drivers::radio::wifi::Station;
use alloc::{format, string::String};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use {
    crate::services::radio::hex, crate::services::radio::known,
    crate::services::radio::known::Known, crate::services::radio::Command,
    crate::services::radio::Credentials, crate::services::radio::Retry,
};

static SAVED: critical_section::Mutex<core::cell::RefCell<known::Snapshot>> =
    critical_section::Mutex::new(core::cell::RefCell::new(known::Snapshot {
        loaded: false,
        enabled: false,
        automatic: true,
        networks: alloc::vec::Vec::new(),
        current: None,
        saving: false,
        error: false,
    }));
pub fn saved() -> known::Snapshot {
    critical_section::with(|cs| SAVED.borrow_ref(cs).clone())
}
pub fn saved_status() -> String {
    let s = saved();
    let mut text = String::new();
    use core::fmt::Write;
    for ssid in &s.networks {
        let _ = writeln!(
            text,
            "@wifi-saved ssid={} current={}",
            hex(ssid),
            u8::from(s.current.as_ref() == Some(ssid))
        );
    }
    let _ = write!(
        text,
        "@ok wifi-saved loaded={} enabled={} automatic={} count={} saving={} error={}",
        u8::from(s.loaded),
        u8::from(s.enabled),
        u8::from(s.automatic),
        s.networks.len(),
        u8::from(s.saving),
        u8::from(s.error)
    );
    text
}
struct Store {
    value: Known,
    persisted: Option<Known>,
    current: Option<alloc::vec::Vec<u8>>,
    error: bool,
}
impl Store {
    fn publish(&self, saving: bool) {
        let snapshot = known::Snapshot {
            loaded: true,
            enabled: self.value.enabled,
            automatic: self.value.automatic,
            networks: self.value.identities(),
            current: if state() == crate::services::radio::WifiState::Connected {
                self.current.clone()
            } else {
                None
            },
            saving,
            error: self.error,
        };
        critical_section::with(|cs| *SAVED.borrow_ref_mut(cs) = snapshot);
    }
    async fn persist(&mut self) -> Result<(), String> {
        if self.persisted.as_ref() == Some(&self.value) {
            self.publish(false);
            return Ok(());
        }
        self.publish(true);
        let mut save = known::Save::new(&self.value);
        let result = loop {
            while crate::services::update::ota::device::active() {
                Timer::after_millis(50).await;
            }
            let (next, result) = crate::platform::runtime::executors::services::run(move || {
                let result = ::fs::global::with(|vfs| save.step(vfs));
                (save, result)
            })
            .await;
            save = next;
            match result {
                Ok(true) => break Ok(()),
                Ok(false) => embassy_futures::yield_now().await,
                Err(error) => break Err(error),
            }
        };
        self.error = result.is_err();
        if result.is_ok() {
            self.persisted = Some(self.value.clone());
        }
        self.publish(false);
        result.map_err(|_| String::from("Wi-Fi preferences could not be saved"))
    }
}

static READY: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);
pub fn busy() -> bool {
    PENDING.load(Ordering::Acquire)
}
static STATE: AtomicU8 = AtomicU8::new(0);
static LINK: critical_section::Mutex<core::cell::Cell<crate::services::radio::WifiLink>> =
    critical_section::Mutex::new(core::cell::Cell::new(
        crate::services::radio::WifiLink::Disconnected,
    ));

pub fn link() -> crate::services::radio::WifiLink {
    critical_section::with(|cs| LINK.borrow(cs).get())
}
fn publish_link(station: Option<&Station<'_>>) {
    let link = match station.filter(|station| station.connected()) {
        Some(station) => crate::services::radio::WifiLink::Connected {
            rssi_dbm: station.rssi(),
        },
        None => crate::services::radio::WifiLink::Disconnected,
    };
    critical_section::with(|cs| LINK.borrow(cs).set(link));
}
static REQUEST: Channel<CriticalSectionRawMutex, Command, 1> = Channel::new();
static DONE: Signal<CriticalSectionRawMutex, String> = Signal::new();
static IPV4: AtomicU32 = AtomicU32::new(0);
static PREFIX: AtomicU8 = AtomicU8::new(0);
static GATEWAY: AtomicU32 = AtomicU32::new(0);
pub fn gateway() -> Option<[u8; 4]> {
    let bytes = GATEWAY.load(Ordering::Acquire).to_be_bytes();
    (state() == crate::services::radio::WifiState::Connected && bytes != [0; 4]).then_some(bytes)
}
static NETWORKS: critical_section::Mutex<
    core::cell::RefCell<alloc::vec::Vec<crate::services::radio::Network>>,
> = critical_section::Mutex::new(core::cell::RefCell::new(alloc::vec::Vec::new()));
pub fn networks() -> alloc::vec::Vec<crate::services::radio::Network> {
    critical_section::with(|cs| NETWORKS.borrow_ref(cs).clone())
}
pub fn state() -> crate::services::radio::WifiState {
    use crate::services::radio::WifiState::*;
    match STATE.load(Ordering::Acquire) {
        0 => Off,
        1 => Idle,
        2 => Scanning,
        3 => Associating,
        4 => Dhcp,
        5 => Connected,
        6 => Fault,
        _ => Reconnecting,
    }
}
pub fn ip() -> Option<[u8; 4]> {
    let bytes = IPV4.load(Ordering::Acquire).to_be_bytes();
    (state() == crate::services::radio::WifiState::Connected && bytes != [0; 4]).then_some(bytes)
}

fn publish(stack: embassy_net::Stack<'_>) {
    if let Some(config) = stack.config_v4().filter(|_| stack.is_link_up()) {
        GATEWAY.store(
            config
                .gateway
                .map_or(0, |ip| u32::from_be_bytes(ip.octets())),
            Ordering::Release,
        );
        PREFIX.store(config.address.prefix_len(), Ordering::Relaxed);
        IPV4.store(
            u32::from_be_bytes(config.address.address().octets()),
            Ordering::Release,
        );
    } else {
        IPV4.store(0, Ordering::Release);
        GATEWAY.store(0, Ordering::Release);
    }
}
pub fn status() -> String {
    let state = match STATE.load(Ordering::Acquire) {
        0 => "off",
        1 => "idle",
        2 => "scanning",
        3 => "associating",
        4 => "dhcp",
        5 => "connected",
        6 => "fault",
        _ => "reconnecting",
    };
    let bytes = IPV4.load(Ordering::Acquire).to_be_bytes();
    let ip = if state == "connected" && bytes != [0; 4] {
        format!(
            "{}.{}.{}.{}/{}",
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
            PREFIX.load(Ordering::Relaxed)
        )
    } else {
        "none".into()
    };
    let gateway = gateway().map_or_else(
        || "none".into(),
        |ip| format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
    );
    format!("@ok wifi-status state={state} ip={ip} gateway={gateway} band=2.4GHz auth=wpa2 ps=off")
}
struct Pending;
impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.store(false, Ordering::Release);
    }
}
pub async fn execute(command: Command) -> String {
    if !READY.load(Ordering::Acquire) {
        return "@err wifi service starting".into();
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return "@err wifi busy".into();
    }
    let _pending = Pending;
    DONE.reset();
    REQUEST.send(command).await;
    DONE.wait().await
}
fn reply(result: Result<String, String>, verb: &str) {
    DONE.signal(result.unwrap_or_else(|e| format!("@err {verb} {e}")));
}

#[embassy_executor::task]
pub async fn network_task(
    stack: embassy_net::Stack<'static>,
    runner: embassy_net::Runner<'static, esp_radio::wifi::Interface>,
) {
    crate::services::radio::device::network::run(stack, runner).await;
}

#[embassy_executor::task]
pub async fn task(
    mut device: esp_hal::peripherals::WIFI<'static>,
    stack: embassy_net::Stack<'static>,
) {
    if !crate::services::power::battery_task::wait_boot().await {
        return;
    }
    while !crate::platform::storage::nor::ready() {
        Timer::after_millis(50).await;
    }
    let loaded =
        crate::platform::runtime::executors::services::run(|| ::fs::global::with(known::load))
            .await;
    let mut store = match loaded {
        Ok(value) => Store {
            persisted: Some(value.clone()),
            value,
            current: None,
            error: false,
        },
        Err(error) => {
            log::warn!("wifi: saved networks unavailable {error}");
            Store {
                value: Known::default(),
                persisted: None,
                current: None,
                error: true,
            }
        }
    };
    store.publish(false);
    // Power and association are separate preferences. With automatic disabled,
    // a remembered power-on still starts an idle station after reboot.
    let mut boot =
        store.value.enabled && !crate::services::power::button_task::shutdown_requested();
    READY.store(true, Ordering::Release);
    loop {
        publish_link(None);
        STATE.store(0, Ordering::Release);
        let (first, reply_first) = if core::mem::take(&mut boot) {
            (Command::WifiOn, false)
        } else {
            (REQUEST.receive().await, true)
        };
        if crate::services::power::button_task::shutdown_requested()
            && !matches!(
                first,
                Command::WifiOff | Command::WifiSuspend | Command::WifiDisconnect
            )
        {
            if reply_first {
                reply(Err("shutdown pending".into()), first.verb());
            }
            continue;
        }
        // Editing saved networks while powered off never starts the PHY.
        let handled = match &first {
            Command::WifiOff => {
                store.value.enabled = false;
                true
            }
            Command::WifiSuspend | Command::WifiDisconnect => true,
            Command::WifiAutomatic(on) => {
                store.value.automatic = *on;
                boot = *on && store.value.enabled && !store.value.identities().is_empty();
                true
            }
            Command::WifiForget(ssid) => {
                store.value.forget(ssid);
                true
            }
            _ => false,
        };
        if handled {
            let result = if matches!(first, Command::WifiSuspend | Command::WifiDisconnect) {
                Ok(())
            } else {
                store.persist().await
            };
            if reply_first {
                reply(
                    result.map(|_| format!("@ok {} state=off", first.verb())),
                    first.verb(),
                );
            }
            continue;
        }
        if let Command::WifiSavedConnect(ssid) = &first {
            if store.value.get(ssid).is_none() {
                if reply_first {
                    reply(Err("network is not saved".into()), first.verb());
                }
                continue;
            }
        }
        // A fresh borrow is made only after the previous controller was dropped.
        let mut station = match Station::new(device.reborrow()) {
            Ok(s) => s,
            Err(e) => {
                STATE.store(6, Ordering::Release);
                if reply_first {
                    reply(Err(e), first.verb());
                }
                continue;
            }
        };
        STATE.store(1, Ordering::Release);
        let mut request = Some((first, reply_first));
        let mut desired: Option<Credentials> = None;
        let mut explicit = false;
        let mut disconnected = false;
        let mut retry = Retry::new();
        loop {
            let command = if let Some(cmd) = request.take() {
                Some(cmd)
            } else {
                match select(REQUEST.receive(), Timer::after_secs(1)).await {
                    Either::First(cmd) => Some((cmd, true)),
                    Either::Second(_) => None,
                }
            };
            // Refresh before a command can await (notably scans), and at the
            // end of the iteration. Idle links are sampled once per second.
            publish_link(Some(&station));
            if let Some((command, needs_reply)) = command {
                let verb = command.verb();
                if crate::services::power::button_task::shutdown_requested()
                    && !matches!(
                        command,
                        Command::WifiOff | Command::WifiSuspend | Command::WifiDisconnect
                    )
                {
                    if needs_reply {
                        reply(Err("shutdown pending".into()), verb);
                    }
                    continue;
                }
                if matches!(command, Command::WifiOff | Command::WifiSuspend) {
                    if command == Command::WifiOff {
                        store.value.enabled = false;
                    }
                    // Suspend used by OTA intentionally does not persist power off.
                    let result = if command == Command::WifiOff {
                        store.persist().await
                    } else {
                        Ok(())
                    };
                    publish_link(None);
                    drop(station);
                    stack.set_config_v4(embassy_net::ConfigV4::None);
                    stack.set_config_v4(embassy_net::ConfigV4::Dhcp(Default::default()));
                    STATE.store(0, Ordering::Release);
                    IPV4.store(0, Ordering::Release);
                    GATEWAY.store(0, Ordering::Release);
                    store.current = None;
                    store.publish(false);
                    critical_section::with(|cs| NETWORKS.borrow_ref_mut(cs).clear());
                    if needs_reply {
                        reply(result.map(|_| format!("@ok {verb} state=off")), verb);
                    }
                    break;
                }
                let command = match command {
                    Command::WifiSavedConnect(ssid) => match store.value.get(&ssid) {
                        Some(credentials) => Command::WifiConnect(credentials),
                        None => {
                            if needs_reply {
                                reply(Err("network is not saved".into()), verb);
                            }
                            continue;
                        }
                    },
                    command => command,
                };
                let result = match command {
                    Command::WifiOn => {
                        store.value.enabled = true;
                        disconnected = false;
                        retry.reset();
                        Ok("@ok wifi-on state=on".into())
                    }
                    Command::WifiAutomatic(on) => {
                        store.value.automatic = on;
                        disconnected = false;
                        retry.reset();
                        if !on && !explicit {
                            desired = None;
                        }
                        Ok(format!("@ok wifi-auto automatic={}", u8::from(on)))
                    }
                    Command::WifiForget(ssid) => {
                        store.value.forget(&ssid);
                        if desired.as_ref().is_some_and(|c| c.ssid == ssid)
                            || store.current.as_ref() == Some(&ssid)
                        {
                            desired = None;
                            explicit = false;
                            store.current = None;
                            let _ =
                                with_timeout(Duration::from_secs(3), station.disconnect()).await;
                            publish_link(Some(&station));
                        }
                        Ok("@ok wifi-forget forgotten".into())
                    }
                    Command::WifiDisconnect => {
                        desired = None;
                        explicit = false;
                        disconnected = true;
                        store.current = None;
                        retry.reset();
                        match with_timeout(Duration::from_secs(3), station.disconnect()).await {
                            Ok(Ok(())) => Ok("@ok wifi-disconnect state=idle".into()),
                            Ok(Err(e)) => Err(e),
                            Err(_) => Err("disconnect timeout".into()),
                        }
                    }
                    Command::WifiScan => {
                        store.value.enabled = true;
                        scan(&mut station).await
                    }
                    Command::WifiConnect(credentials) => {
                        desired = None;
                        explicit = true;
                        disconnected = false;
                        store.current = None;
                        retry.reset();
                        store.value.enabled = true;
                        let _ = with_timeout(Duration::from_secs(3), station.disconnect()).await;
                        publish_link(Some(&station));
                        stack.set_config_v4(embassy_net::ConfigV4::None);
                        stack.set_config_v4(embassy_net::ConfigV4::Dhcp(Default::default()));
                        match station.configure(&credentials) {
                            Err(e) => Err(e),
                            Ok(()) => {
                                desired = Some(credentials);
                                let result = associate(&mut station, stack).await;
                                if result.is_err() {
                                    retry.failed(Instant::now().as_millis());
                                }
                                result.map(|()| format!("@ok {verb} connected"))
                            }
                        }
                    }

                    _ => Err("invalid Wi-Fi request".into()),
                };
                if station.connected() && stack.is_config_up() {
                    if let Some(credentials) = &desired {
                        store.current = Some(credentials.ssid.clone());
                        store.value.remember(credentials.clone());
                    }
                }
                STATE.store(
                    if station.connected() && stack.is_config_up() {
                        5
                    } else if desired.is_some() {
                        7
                    } else {
                        1
                    },
                    Ordering::Release,
                );
                publish_link(Some(&station));
                publish(stack);
                let saved = store.persist().await;
                if needs_reply {
                    reply(result.and_then(|r| saved.map(|_| r)), verb);
                }
            } else {
                if station.connected() {
                    STATE.store(if stack.is_config_up() { 5 } else { 4 }, Ordering::Release);
                    if stack.is_config_up() {
                        if let Some(credentials) = &desired {
                            store.current = Some(credentials.ssid.clone());
                            store.value.remember(credentials.clone());
                            let _ = store.persist().await;
                        }
                        retry.reset();
                    }
                } else if retry.due(Instant::now().as_millis())
                    && !crate::services::update::ota::device::active()
                    && !crate::services::update::task::busy()
                    && !crate::services::power::button_task::shutdown_requested()
                {
                    store.current = None;
                    // A manual disconnect lasts for this boot. Automatic choice
                    // uses visible saved networks in most-recent-success order.
                    if !explicit
                        && !disconnected
                        && store.value.enabled
                        && store.value.automatic
                        && !store.value.identities().is_empty()
                    {
                        if scan(&mut station).await.is_ok() {
                            desired = store.value.available(&networks());
                        }
                    }
                    if let Some(credentials) = &desired {
                        STATE.store(7, Ordering::Release);
                        let configured = station.configure(credentials);
                        match configured {
                            Ok(()) => match associate(&mut station, stack).await {
                                Ok(()) => {
                                    retry.reset();
                                    store.current = Some(credentials.ssid.clone());
                                    store.value.remember(credentials.clone());
                                    let _ = store.persist().await;
                                    log::info!("wifi: connected, saved-network DHCP ready");
                                }
                                Err(_) => {
                                    retry.failed(Instant::now().as_millis());
                                    STATE.store(7, Ordering::Release);
                                }
                            },
                            Err(_) => {
                                retry.failed(Instant::now().as_millis());
                                STATE.store(7, Ordering::Release);
                            }
                        }
                    } else {
                        retry.failed(Instant::now().as_millis());
                        STATE.store(1, Ordering::Release);
                    }
                }
                publish_link(Some(&station));
                publish(stack);
                store.publish(false);
            }
        }
    }
}

async fn scan(station: &mut Station<'_>) -> Result<String, String> {
    STATE.store(2, Ordering::Release);
    let networks = match with_timeout(Duration::from_secs(8), station.scan()).await {
        Ok(result) => result?,
        Err(_) => return Err("scan timeout".into()),
    };
    let snapshot = networks
        .iter()
        .map(|ap| crate::services::radio::Network {
            ssid: ap.ssid.clone(),
            channel: ap.channel,
            rssi: ap.rssi,
            auth: ap.auth.clone(),
        })
        .collect();
    critical_section::with(|cs| *NETWORKS.borrow_ref_mut(cs) = snapshot);
    let mut text = String::new();
    use core::fmt::Write;
    for ap in &networks {
        let _ = writeln!(
            text,
            "@wifi-ap ssid={} ch={} rssi={} auth={}",
            hex(&ap.ssid),
            ap.channel,
            ap.rssi,
            ap.auth
        );
    }
    let _ = write!(text, "@ok wifi-scan count={} band=2.4GHz", networks.len());
    Ok(text)
}

async fn associate(station: &mut Station<'_>, stack: embassy_net::Stack<'_>) -> Result<(), String> {
    publish_link(Some(station));
    STATE.store(3, Ordering::Release);
    match with_timeout(Duration::from_secs(10), station.connect()).await {
        Ok(Ok(())) => (),
        Ok(Err(e)) => {
            publish_link(Some(station));
            return Err(e);
        }
        Err(_) => {
            let _ = with_timeout(Duration::from_secs(2), station.disconnect()).await;
            publish_link(Some(station));
            return Err("association timeout".into());
        }
    }
    publish_link(Some(station));
    STATE.store(4, Ordering::Release);
    let configured = with_timeout(Duration::from_secs(15), stack.wait_config_up()).await;
    publish_link(Some(station));
    configured.map_err(|_| String::from("DHCP timeout"))?;
    STATE.store(5, Ordering::Release);
    Ok(())
}
