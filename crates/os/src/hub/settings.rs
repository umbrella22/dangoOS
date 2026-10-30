//! User preferences and the settings navigation/operation contract.
use super::Settings;
use alloc::{string::String, vec::Vec};

pub const PATH: &str = "/data/settings/device.json";
/// SSID is a byte identity. Only its label is formatted for LVGL's C strings;
/// selection and association always retain the original bytes.
pub fn ssid_label(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "隐藏网络".into();
    }
    match core::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => text.into(),
        _ => alloc::format!("网络 {}", crate::radio::hex(bytes)),
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Root,
    Display,
    Character,
    Feedback,
    Wireless,
    Wifi,
    KnownNetwork,
    Password,
    Camera,
    Audio,
    Storage,
    System,
    Update,
    Proxy,
}
impl Page {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Root => "设置",
            Self::Display => "显示与电源",
            Self::Character => "更换团子",
            Self::Feedback => "灯光与震动",
            Self::Wireless => "无线连接",
            Self::Wifi => "Wi-Fi",
            Self::KnownNetwork => "已保存网络",
            Self::Password => "连接网络",
            Self::Camera => "相机",
            Self::Audio => "声音",
            Self::Storage => "存储",
            Self::System => "关于设备",
            Self::Update => "系统更新",
            Self::Proxy => "更新代理",
        }
    }
    pub const fn back(self) -> Self {
        match self {
            Self::Password | Self::KnownNetwork => Self::Wifi,
            Self::Wifi => Self::Wireless,
            Self::Proxy => Self::Update,
            _ => Self::Root,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Radio(crate::radio::Command),
    ScreenOff,
    Update(crate::update::Command),
    SelectSkin(String),
}

/// Input text never appears in Debug output. Successful credentials are saved
/// by the Wi-Fi service, separately from public device preferences.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Password(pub String);
impl core::fmt::Debug for Password {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Password(REDACTED)")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub skins: Vec<super::skins::Choice>,
    pub selected_skin: String,
    /// Target whose loading indicator must be painted before resource I/O.
    pub loading_skin: Option<String>,
    pub networks: Vec<crate::radio::Network>,
    pub wifi_saved: crate::radio::known::Snapshot,
    pub selected: Option<crate::radio::Network>,
    pub password: Password,
    pub password_hidden: bool,
    pub update: crate::update::Snapshot,
    pub proxy_address: String,
    pub info: Info,
    pub action: Option<Action>,
    pub message: Option<&'static str>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub wifi: crate::radio::WifiState,
    pub bluetooth: crate::radio::BluetoothState,
    pub ip: Option<[u8; 4]>,
    pub busy: bool,
    pub audio: crate::audio::State,
    pub data_free_kib: u32,
    pub data_total_kib: u32,
    pub storage_ready: bool,
    pub saved: bool,
    pub saving: bool,
    pub save_error: bool,
    pub candidate: bool,
    pub boot_slot: Option<u8>,
}

pub fn encode(settings: Settings) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"schema":1,"audio_volume":settings.audio_volume,"audio_muted":settings.audio_muted,"brightness":settings.brightness,"lights":settings.lights,"haptics":settings.haptics,"camera_mirror":settings.camera_mirror,"jpeg_quality":settings.jpeg_quality,"auto_update_check":settings.auto_update_check,
        "update_proxy_enabled":settings.update_proxy_enabled,
        "update_proxy":settings.update_proxy.map(crate::update::HttpProxy::address)})).expect("preference scalars")
}
pub fn decode(bytes: &[u8]) -> Result<Settings, &'static str> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "preferences JSON")?;
    if v["schema"].as_u64() != Some(1) {
        return Err("preferences schema");
    }
    let brightness = v["brightness"]
        .as_u64()
        .filter(|&n| (10..=100).contains(&n))
        .ok_or("preferences brightness")? as u8;
    let quality = v["jpeg_quality"]
        .as_u64()
        .filter(|&n| [75, 85, 95].contains(&n))
        .ok_or("preferences quality")? as u8;
    let proxy = match v.get("update_proxy") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(crate::update::HttpProxy::parse(value.as_str().ok_or("preferences proxy")?)?),
    };
    let proxy_enabled = match v.get("update_proxy_enabled") {
        None => false,
        Some(value) => value.as_bool().ok_or("preferences proxy enabled")?,
    };
    if proxy_enabled && proxy.is_none() { return Err("preferences proxy missing"); }
    Ok(Settings {
        brightness,
        lights: v["lights"].as_bool().ok_or("preferences lights")?,
        haptics: v["haptics"].as_bool().ok_or("preferences haptics")?,
        camera_mirror: v["camera_mirror"].as_bool().ok_or("preferences mirror")?,
        jpeg_quality: quality,
        audio_volume: match v.get("audio_volume") { None=>50,Some(value)=>value.as_u64().filter(|v|*v<=100).ok_or("preferences audio volume")? as u8 },
        audio_muted: match v.get("audio_muted") { None=>false,Some(value)=>value.as_bool().ok_or("preferences audio mute")? },
        auto_update_check: match v.get("auto_update_check") {
            None => true,
            Some(value) => value.as_bool().ok_or("preferences update check")?,
        },
        update_proxy_enabled: proxy_enabled,
        update_proxy: proxy,
    })
}
pub fn load(vfs: &mut ::fs::Vfs) -> Result<Option<Settings>, ::fs::Errno> {
    match vfs.read_file(PATH) {
        Ok(bytes) => decode(&bytes).map(Some).map_err(|_| ::fs::Errno::INVAL),
        Err(::fs::Errno::NOENT) => Ok(None),
        Err(e) => Err(e),
    }
}
const TEMP: &str = "/data/settings/.device-pending";

enum SaveStep {
    Prepare,
    Write(i32),
    Commit,
    Done,
}

/// Drive to completion, releasing the VFS between steps so the device can
/// service input and render. Readers see the old preferences until Commit.
pub struct Save {
    bytes: Vec<u8>,
    next: SaveStep,
}
impl Save {
    pub fn new(settings: Settings) -> Self {
        Self {
            bytes: encode(settings),
            next: SaveStep::Prepare,
        }
    }

    /// Returns true after the atomic replacement. Write errors still close the
    /// open descriptor; the old preference file remains untouched.
    pub fn step(&mut self, vfs: &mut ::fs::Vfs) -> Result<bool, ::fs::Errno> {
        let next = core::mem::replace(&mut self.next, SaveStep::Done);
        self.next = match next {
            SaveStep::Prepare => {
                vfs.create_dir_all("/data/settings")?;
                SaveStep::Write(vfs.open(TEMP, ::fs::OpenFlags::WRITE)?)
            }
            SaveStep::Write(fd) => {
                let written = vfs.write(fd, &self.bytes).and_then(|n| {
                    if n == self.bytes.len() {
                        Ok(())
                    } else {
                        Err(::fs::Errno::NOSPC)
                    }
                });
                let closed = vfs.close(fd);
                written.and(closed)?;
                SaveStep::Commit
            }
            SaveStep::Commit => {
                vfs.rename(TEMP, PATH)?;
                SaveStep::Done
            }
            SaveStep::Done => SaveStep::Done,
        };
        Ok(matches!(self.next, SaveStep::Done))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn save(vfs: &mut ::fs::Vfs, value: Settings) -> Result<(), ::fs::Errno> {
        let mut save = Save::new(value);
        while !save.step(vfs)? {}
        Ok(())
    }
    #[test]
    fn preferences_persist_atomically_and_exclude_radio_secrets() {
        let mut vfs = ::fs::Vfs::default();
        let (volume, _) = ::fs::Fs::mount_or_format(
            alloc::boxed::Box::new(::fs::RamDevice::new(4096, 128)),
            ::fs::Geometry::flash(128),
        )
        .unwrap();
        vfs.mount("data", volume);
        let mut s = Settings::new();
        s.brightness = 44;
        s.haptics = false;
        s.camera_mirror = true;
        s.jpeg_quality = 95;
        s.audio_volume = 27;
        s.audio_muted = true;
        s.update_proxy = Some(crate::update::HttpProxy::parse("10.26.104.1:7777").unwrap());
        s.update_proxy_enabled = true;
        assert_eq!(load(&mut vfs).unwrap(), None);
        save(&mut vfs, s).unwrap();
        assert_eq!(load(&mut vfs).unwrap(), Some(s));
        let old = s;
        s.brightness = 61;
        let mut saving = Save::new(s);
        assert!(!saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(old));
        assert!(!saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(old));
        assert!(saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(s));
        assert!(vfs.stat("/data/settings/.device-pending").is_err());
        let bytes = vfs.read_file(PATH).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("password"));
        let mut v = serde_json::from_slice::<serde_json::Value>(&encode(s)).unwrap();
        v["brightness"] = serde_json::json!(9);
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn update_preferences_default_old_files_and_reject_invalid_proxy() {
        let mut v = serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
        for key in ["auto_update_check", "update_proxy_enabled", "update_proxy", "audio_volume", "audio_muted"] { v.as_object_mut().unwrap().remove(key); }
        assert_eq!(decode(&serde_json::to_vec(&v).unwrap()).unwrap(), Settings::new());
        v["update_proxy_enabled"] = true.into();
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        v["update_proxy"] = "10.26.104.1:7777".into();
        assert!(decode(&serde_json::to_vec(&v).unwrap()).unwrap().update_proxy_enabled);
        for bad in ["http://10.26.104.1:7777", "0.0.0.0:7777", "10.26.104.1:0", "10.26.104.1:65536", "10.26.104.1:7777\n@mosaico reboot"] {
            v["update_proxy"] = bad.into();
            assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        }
    }
    #[test]
    fn audio_preferences_reject_invalid_volume_and_mute_types() {
        for bad in [serde_json::json!(101),serde_json::json!(-1),serde_json::json!(50.5),serde_json::json!("50"),serde_json::Value::Null] {
            let mut v=serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
            v["audio_volume"]=bad;
            assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        }
        let mut v=serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
        v["audio_muted"]=serde_json::json!(1);
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn ssid_display_does_not_change_the_selected_identity() {
        let bytes = b"a\0b\xff".to_vec();
        assert_eq!(ssid_label(&bytes), "网络 610062ff");
        assert_eq!(ssid_label(b"a b"), "a b");
        assert_eq!(ssid_label("中文网络".as_bytes()), "中文网络");
        let credentials =
            crate::radio::Credentials::wpa2(bytes.clone(), b"test-key8".to_vec()).unwrap();
        assert_eq!(credentials.ssid, bytes);
    }
}
