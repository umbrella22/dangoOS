//! User preferences and the settings navigation/operation contract.
use alloc::{string::String, vec::Vec};

/// SSID is a byte identity. Only its label is formatted for LVGL's C strings;
/// selection and association always retain the original bytes.
pub fn ssid_label(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "隐藏网络".into();
    }
    match core::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => text.into(),
        _ => alloc::format!("网络 {}", crate::services::radio::hex(bytes)),
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
    pub skins: Vec<crate::companion::skins::Choice>,
    pub selected_skin: String,
    /// Target whose loading indicator must be painted before resource I/O.
    pub loading_skin: Option<String>,
    pub networks: Vec<crate::services::radio::Network>,
    pub wifi_saved: crate::services::radio::known::Snapshot,
    pub selected: Option<crate::services::radio::Network>,
    pub password: Password,
    pub password_hidden: bool,
    pub update: crate::services::update::Snapshot,
    pub proxy_address: String,
    pub info: Info,
    pub action: Option<crate::services::settings::Command>,
    pub message: Option<&'static str>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub wifi: crate::services::radio::WifiState,
    pub bluetooth: crate::services::radio::BluetoothState,
    pub ip: Option<[u8; 4]>,
    pub busy: bool,
    pub audio: crate::services::audio::State,
    pub data_free_kib: u32,
    pub data_total_kib: u32,
    pub storage_ready: bool,
    pub saved: bool,
    pub saving: bool,
    pub save_error: bool,
    pub candidate: bool,
    pub boot_slot: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssid_display_does_not_change_the_selected_identity() {
        let bytes = b"a\0b\xff".to_vec();
        assert_eq!(ssid_label(&bytes), "网络 610062ff");
        assert_eq!(ssid_label(b"a b"), "a b");
        assert_eq!(ssid_label("中文网络".as_bytes()), "中文网络");
        let credentials =
            crate::services::radio::Credentials::wpa2(bytes.clone(), b"test-key8".to_vec())
                .unwrap();
        assert_eq!(credentials.ssid, bytes);
    }
}
