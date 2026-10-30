//! Wireless service commands. HAL types and controller quirks stay in drivers.
//! SSIDs use hex on the console so spaces and non-UTF-8 bytes survive framing.
use alloc::{string::String, vec::Vec};
use core::fmt;

pub mod lifecycle;
pub mod known;

#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum WifiState{#[default]Off,Idle,Scanning,Associating,Dhcp,Connected,Fault,Reconnecting}
impl WifiState{pub const fn label(self)->&'static str{match self{Self::Off=>"已关闭",Self::Idle=>"未连接",Self::Scanning=>"正在搜索",Self::Associating=>"正在连接",Self::Dhcp=>"正在获取地址",Self::Connected=>"已连接",Self::Fault=>"连接失败",Self::Reconnecting=>"正在重连"}}}
impl WifiState {
    pub const fn enabled(self) -> bool {
        !matches!(self, Self::Off | Self::Fault)
    }
}
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum BluetoothState{#[default]Off,Starting,Discoverable,Connected,Streaming,Fault}
impl BluetoothState{
 pub const fn label(self)->&'static str{match self{Self::Off=>"已关闭",Self::Starting=>"正在启动",Self::Discoverable=>"可被发现",Self::Connected=>"已连接",Self::Streaming=>"正在播放",Self::Fault=>"启动失败"}}
 pub const fn name(self)->&'static str{match self{Self::Off=>"off",Self::Starting=>"starting",Self::Discoverable=>"discoverable",Self::Connected=>"connected",Self::Streaming=>"streaming",Self::Fault=>"fault"}}
 pub const fn enabled(self)->bool{!matches!(self,Self::Off|Self::Fault)}
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct Network{pub ssid:Vec<u8>,pub channel:u8,pub rssi:i8,pub auth:String}

/// Read-only service state shared by native status pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub wifi: WifiState,
    pub bluetooth: BluetoothState,
    pub ip: Option<[u8; 4]>,
}
impl Snapshot {
    pub const fn new() -> Self {
        Self {
            wifi: WifiState::Off,
            bluetooth: BluetoothState::Off,
            ip: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub ssid: Vec<u8>,
    pub password: Vec<u8>,
}
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(REDACTED)")
    }
}
impl Credentials{
 pub fn wpa2(ssid:Vec<u8>,password:Vec<u8>)->Result<Self,&'static str>{
  if ssid.is_empty()||ssid.len()>32{return Err("SSID requires 1..32 bytes");}
  if !(8..=63).contains(&password.len())||!password.iter().all(|c|(32..=126).contains(c)){return Err("WPA2 requires 8..63 printable ASCII password bytes");}
  Ok(Self{ssid,password})
 }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    WifiStatus,
    WifiOn,
    WifiScan,
    WifiConnect(Credentials),
    WifiSavedConnect(Vec<u8>),
    WifiForget(Vec<u8>),
    WifiAutomatic(bool),
    WifiSaved,
    WifiDisconnect,
    WifiOff,
    /// OTA shutdown is temporary and must preserve the user's power preference.
    WifiSuspend,
    WifiProbe { host: String, port: u16 },
    BtStatus,
    BtOn,
    BtOff,
}
impl Command {
    pub fn parse(verb: &str, args: &[&str]) -> Result<Self, &'static str> {
        Ok(match (verb, args) {
            ("wifi-status", []) => Self::WifiStatus,
            ("wifi-on", []) => Self::WifiOn,
            ("wifi-scan", []) => Self::WifiScan,
            ("wifi-disconnect", []) => Self::WifiDisconnect,
            ("wifi-off", []) => Self::WifiOff,
            ("wifi-saved", []) => Self::WifiSaved,
            ("wifi-saved-connect", [ssid]) => Self::WifiSavedConnect(unhex(ssid, 32)?),
            ("wifi-forget", [ssid]) => Self::WifiForget(unhex(ssid, 32)?),
            ("wifi-auto", ["on"]) => Self::WifiAutomatic(true),
            ("wifi-auto", ["off"]) => Self::WifiAutomatic(false),
            ("bt-status", []) => Self::BtStatus,
            ("bt-on", []) => Self::BtOn,
            ("bt-off", []) => Self::BtOff,
            ("wifi-connect", [ssid, password]) => {
                let ssid = unhex(ssid, 32)?;
                let password = unhex(password, 63)?;
                // WPA2 passphrases are 8..63 printable ASCII bytes. Raw 256-bit
                // PSKs and SAE need a distinct backend contract, not truncation.
                Self::WifiConnect(Credentials::wpa2(ssid,password)?)
            }
            ("wifi-probe", [host, port]) => {
                // This diagnostic sends a fixed HTTP/1.0 GET to a host/port.
                // Restrict the wire host field to prevent request injection.
                if host.is_empty()
                    || host.len() > 253
                    || !host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
                {
                    return Err("probe expects a DNS host or IPv4 address");
                }
                let port = port
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0)
                    .ok_or("probe expects port 1..65535")?;
                Self::WifiProbe {
                    host: (*host).into(),
                    port,
                }
            }
            _ => return Err("invalid wireless command"),
        })
    }
    pub const fn status(&self) -> bool {
        matches!(self, Self::WifiStatus | Self::WifiSaved | Self::BtStatus)
    }
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::WifiStatus => "wifi-status",
            Self::WifiOn => "wifi-on",
            Self::WifiScan => "wifi-scan",
            Self::WifiConnect(_) => "wifi-connect",
            Self::WifiSavedConnect(_) => "wifi-saved-connect",
            Self::WifiForget(_) => "wifi-forget",
            Self::WifiAutomatic(_) => "wifi-auto",
            Self::WifiSaved => "wifi-saved",
            Self::WifiDisconnect => "wifi-disconnect",
            Self::WifiOff => "wifi-off",
            Self::WifiSuspend => "wifi-suspend",
            Self::WifiProbe { .. } => "wifi-probe",
            Self::BtStatus => "bt-status",
            Self::BtOn => "bt-on",
            Self::BtOff => "bt-off",
        }
    }
}

fn unhex(text: &str, max: usize) -> Result<Vec<u8>, &'static str> {
    if text.len() % 2 != 0 || text.len() > max * 2 {
        return Err("invalid hex field length");
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let nibble = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            Ok(nibble(pair[0]).ok_or("invalid hex field")? * 16
                + nibble(pair[1]).ok_or("invalid hex field")?)
        })
        .collect()
}
pub fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Bounded reconnect delays; a user disconnect resets desired connectivity.
pub struct Retry {
    failures: u8,
    deadline_ms: u64,
}
impl Retry {
    pub const fn new() -> Self {
        Self {
            failures: 0,
            deadline_ms: 0,
        }
    }
    pub fn reset(&mut self) {
        self.failures = 0;
        self.deadline_ms = 0;
    }
    pub fn failed(&mut self, now_ms: u64) {
        let seconds = (1u64 << self.failures.min(5)).min(30);
        self.failures = self.failures.saturating_add(1);
        self.deadline_ms = now_ms.saturating_add(seconds * 1000);
    }
    pub fn due(&self, now_ms: u64) -> bool {
        now_ms >= self.deadline_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_preserve_bytes_and_debug_never_exposes_them() {
        let cmd = Command::parse("wifi-connect", &["612062ff", "70617373776f7264"]).unwrap();
        let Command::WifiConnect(credentials) = &cmd else {
            panic!()
        };
        assert_eq!(credentials.ssid, [b'a', b' ', b'b', 255]);
        assert_eq!(credentials.password, b"password");
        assert_eq!(
            alloc::format!("{cmd:?}"),
            "WifiConnect(Credentials(REDACTED))"
        );
    }
    #[test]
    fn malformed_or_oversized_credentials_fail_without_echoing_input() {
        for (ssid, key) in [
            ("0", "70617373776f7264"),
            ("gg", "70617373776f7264"),
            ("", "70617373776f7264"),
            ("61", "00"),
            ("61", "0102030405060708"),
        ] {
            assert!(Command::parse("wifi-connect", &[ssid, key]).is_err());
        }
        assert!(Command::parse("wifi-connect", &[&"61".repeat(33), &"61".repeat(8)]).is_err());
        assert!(Command::parse("wifi-connect", &[&"61".repeat(32), &"61".repeat(63)]).is_ok());
        assert!(Command::parse("wifi-connect", &["61", &"61".repeat(64)]).is_err());
    }
    #[test]
    fn probe_fields_cannot_inject_http_headers_or_overflow_the_port() {
        assert!(Command::parse("wifi-probe", &["192.168.1.1", "80"]).is_ok());
        for (host, port) in [
            ("example.com\r\nHeader:value", "80"),
            ("host/path", "80"),
            ("localhost", "0"),
            ("localhost", "65536"),
        ] {
            assert!(Command::parse("wifi-probe", &[host, port]).is_err());
        }
    }
    #[test]
    fn reconnect_waits_are_bounded_and_reset_after_recovery() {
        let mut retry = Retry::new();
        let mut now = 0;
        for delay in [1, 2, 4, 8, 16, 30, 30] {
            retry.failed(now);
            assert!(!retry.due(now + delay * 1000 - 1));
            now += delay * 1000;
            assert!(retry.due(now));
        }
        retry.reset();
        assert!(retry.due(0));
    }
}
