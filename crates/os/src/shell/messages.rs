//! Messages routed by the shell application.
use crate::apps::lifecycle::uninstall::Identity;
use crate::shell::navigation::gesture::SystemGesture;
use crate::shell::navigation::route::AppId;
use alloc::string::String;
/// Messages consumed by the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppMsg {
    /// The splash finished or was skipped; switch to the fox.
    SplashDone,
    /// Advance through the six AI states (host preview and tests).
    Next,
    /// A launcher tile was tapped.
    Open(AppId),
    OpenLua(Identity),
    ManageAsk(Identity),
    ManageCancel,
    ManageConfirm,
    /// A system edge gesture (from the UI task, not LVGL).
    Gesture(SystemGesture),
    /// Settings: panel brightness, percent.
    Brightness(i32),
    AudioVolume(i32),
    AudioMute(bool),
    /// Settings: interaction-board lights.
    Lights(bool),
    /// Settings: vibration motor.
    Haptics(bool),
    CameraCapture,
    CameraRetry,
    SettingsPage(crate::shell::pages::settings::model::Page),
    SettingsBack,
    SelectSkin(String),
    CameraMirror(bool),
    JpegQuality(u8),
    WifiScan,
    WifiChoose(crate::services::radio::Network),
    WifiPassword(crate::shell::pages::settings::model::Password),
    WifiPasswordHidden(bool),
    AutoUpdateCheck(bool),
    UpdateProxyToggle(bool),
    UpdateProxyText(String),
    UpdateProxyApply,
    UpdateProxySet(Option<crate::services::update::HttpProxy>),
    Update(crate::services::update::Command),
    WifiConnect,
    WifiSavedConnect,
    WifiForget,
    WifiReenterPassword,
    WifiAutomatic(bool),
    Wifi(bool),
    Bluetooth(bool),
    ScreenOff,
    /// Timer: select a preset (index into `PRESETS_MIN`).
    TimerPreset(usize),
    /// Timer: start, pause or resume.
    TimerToggle,
    /// Timer: back to the full preset.
    TimerReset,
}
