use crate::drivers::radio::wifi::Station;
use alloc::{format, string::String};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{with_timeout, Duration, Instant, Timer};
use os::radio::{
    hex,
    known::{self, Known},
    Command, Credentials, Retry,
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
            current: if state() == os::radio::WifiState::Connected {
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
            while crate::ota_runtime::active() {
                Timer::after_millis(50).await;
            }
            let (next, result) = crate::service_executor::run(move || {
                let result = ::fs::global::with(|vfs| save.step(vfs));
                (save, result)
            }).await;
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
static REQUEST: Channel<CriticalSectionRawMutex, Command, 1> = Channel::new();
static DONE: Signal<CriticalSectionRawMutex, String> = Signal::new();
static IPV4: AtomicU32 = AtomicU32::new(0);
static PREFIX: AtomicU8 = AtomicU8::new(0);
static GATEWAY: AtomicU32 = AtomicU32::new(0);
pub fn gateway() -> Option<[u8; 4]> {
    let bytes = GATEWAY.load(Ordering::Acquire).to_be_bytes();
    (state() == os::radio::WifiState::Connected && bytes != [0; 4]).then_some(bytes)
}
static NETWORKS: critical_section::Mutex<core::cell::RefCell<alloc::vec::Vec<os::radio::Network>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(alloc::vec::Vec::new()));
pub fn networks() -> alloc::vec::Vec<os::radio::Network> {
    critical_section::with(|cs| NETWORKS.borrow_ref(cs).clone())
}
pub fn state() -> os::radio::WifiState {
    use os::radio::WifiState::*;
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
    (state() == os::radio::WifiState::Connected && bytes != [0; 4]).then_some(bytes)
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
    super::network::run(stack, runner).await;
}

#[embassy_executor::task]
pub async fn task(
    mut device: esp_hal::peripherals::WIFI<'static>,
    stack: embassy_net::Stack<'static>,
) {
    if !super::super::battery::wait_boot().await { return; }
    while !crate::drivers::storage::ready() {
        Timer::after_millis(50).await;
    }
    let loaded = crate::service_executor::run(|| ::fs::global::with(known::load)).await;
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
    let mut boot = store.value.enabled && !super::super::buttons::shutdown_requested();
    READY.store(true, Ordering::Release);
    loop {
        STATE.store(0, Ordering::Release);
        let (first, reply_first) = if core::mem::take(&mut boot) {
            (Command::WifiOn, false)
        } else {
            (REQUEST.receive().await, true)
        };
        if super::super::buttons::shutdown_requested()
            && !matches!(first, Command::WifiOff | Command::WifiSuspend | Command::WifiDisconnect) {
            if reply_first { reply(Err("shutdown pending".into()), first.verb()); }
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
            if let Some((command, needs_reply)) = command {
                let verb = command.verb();
                if super::super::buttons::shutdown_requested()
                    && !matches!(command, Command::WifiOff | Command::WifiSuspend | Command::WifiDisconnect) {
                    if needs_reply { reply(Err("shutdown pending".into()), verb); }
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
                    Command::WifiProbe { host, port } => probe(stack, &host, port).await,
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
                    && !crate::ota_runtime::active()
                    && !super::super::update::busy()
                    && !super::super::buttons::shutdown_requested()
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
        .map(|ap| os::radio::Network {
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
    STATE.store(3, Ordering::Release);
    match with_timeout(Duration::from_secs(10), station.connect()).await {
        Ok(Ok(())) => (),
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            let _ = with_timeout(Duration::from_secs(2), station.disconnect()).await;
            return Err("association timeout".into());
        }
    }
    STATE.store(4, Ordering::Release);
    with_timeout(Duration::from_secs(15), stack.wait_config_up())
        .await
        .map_err(|_| String::from("DHCP timeout"))?;
    STATE.store(5, Ordering::Release);
    Ok(())
}

/// Bounded DNS + TCP + HTTP data-path diagnostic, with no response body logs.
async fn probe(stack: embassy_net::Stack<'_>, host: &str, port: u16) -> Result<String, String> {
    if !stack.is_link_up() || !stack.is_config_up() {
        return Err("network is not configured".into());
    }
    with_timeout(Duration::from_secs(12), async {
        let addresses = stack
            .dns_query(host, embassy_net::dns::DnsQueryType::A)
            .await
            .map_err(|_| "DNS failed")?;
        let address = *addresses.first().ok_or("no IPv4 address")?;
        let mut rx = [0; 2048];
        let mut tx = [0; 1024];
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(5)));
        socket
            .connect((address, port))
            .await
            .map_err(|_| "TCP connect failed")?;
        let request = format!("GET / HTTP/1.0\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
        let mut sent = 0;
        while sent < request.len() {
            let n = socket
                .write(&request.as_bytes()[sent..])
                .await
                .map_err(|_| "TCP write failed")?;
            if n == 0 {
                return Err("TCP write made no progress");
            }
            sent += n;
        }
        let mut bytes = 0;
        let mut buffer = [0; 512];
        let mut header = [0; 12];
        let mut prefix_len = 0;
        let mut crc = !0u32;
        loop {
            let n = socket
                .read(&mut buffer)
                .await
                .map_err(|_| "TCP read failed")?;
            if n == 0 {
                break;
            }
            let prefix = n.min(header.len() - prefix_len);
            header[prefix_len..prefix_len + prefix].copy_from_slice(&buffer[..prefix]);
            prefix_len += prefix;
            for &byte in &buffer[..n] {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
                }
            }
            bytes += n;
            if bytes >= 32768 {
                return Err("HTTP response exceeds diagnostic limit");
            }
        }
        if prefix_len < 12
            || !header.starts_with(b"HTTP/1.")
            || !header[9..12].iter().all(u8::is_ascii_digit)
        {
            return Err("invalid HTTP response");
        }
        let status = core::str::from_utf8(&header[9..12]).map_err(|_| "invalid HTTP status")?;
        Ok(format!(
            "@ok wifi-probe bytes={bytes} crc={:08x} http={status}",
            !crc
        ))
    })
    .await
    .map_err(|_| String::from("probe timeout"))?
    .map_err(String::from)
}
