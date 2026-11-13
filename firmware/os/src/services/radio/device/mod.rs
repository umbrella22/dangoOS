//! Wireless controllers run outside VFS/UI locks. ScreenOff only sleeps the
//! panel; it does not suspend these tasks or claim radio/MCU low power.
use crate::services::radio::Command;
use alloc::{format, string::String};
pub mod classic;
mod network;
pub mod wifi;

pub async fn execute(command: Command) -> String {
    if command.status() {
        let status = match command {
            Command::WifiStatus => wifi::status(),
            Command::WifiSaved => wifi::saved_status(),
            _ => classic::status(),
        };
        return format!(
            "{status} internal_free={}",
            esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into())
        );
    }
    if crate::services::power::button_task::shutdown_requested() {
        return "@err wireless shutdown pending".into();
    }
    if crate::services::update::boot::device::active() || crate::services::update::task::busy() {
        return "@err wireless OTA busy".into();
    }
    if crate::services::voice::device::busy() && !crate::services::voice::device::quiesce().await {
        return "@err wireless voice shutdown pending".into();
    }
    match command {
        Command::BtOn => classic::execute(super::lifecycle::BluetoothAction::On).await,
        Command::BtOff => classic::execute(super::lifecycle::BluetoothAction::Off).await,
        Command::BtRestart => classic::execute(super::lifecycle::BluetoothAction::Restart).await,
        _ => wifi::execute(command).await,
    }
}

/// Both OTA transports release controllers before NOR writes. Leave them off
/// until restart or an explicit command, so reconnect cannot race installation.
pub async fn quiesce() -> bool {
    if !crate::services::voice::device::quiesce().await {
        return false;
    }
    let wifi = wifi::execute(Command::WifiSuspend).await;
    let bluetooth = classic::execute(super::lifecycle::BluetoothAction::Off).await;
    let audio = crate::services::audio::task::quiesce().await;
    wifi.starts_with("@ok") && bluetooth.starts_with("@ok") && audio
}
