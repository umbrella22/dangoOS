//! Wireless settings page.
use super::components::{entry, list, note};
use crate::shell::{
    components::page::{font_20, row, switch, text},
    messages::AppMsg,
    pages::settings::model::{Page, Session},
    theme::tokens::MUTED,
};
use alloc::format;
use rs_lvgl_ui::{Key, View};

pub(super) fn view(session: &Session) -> View<AppMsg> {
    let info = session.info;
    let body = list("wireless-controls")
        .child(entry(
            Key::Named("wifi"),
            "Wi-Fi",
            format!("{}  ›", info.wifi.label()),
            AppMsg::SettingsPage(Page::Wifi),
        ))
        .child(row(
            "bluetooth",
            "蓝牙",
            switch(
                "switch",
                matches!(
                    info.bluetooth,
                    crate::services::radio::BluetoothState::Discoverable
                        | crate::services::radio::BluetoothState::Connected
                        | crate::services::radio::BluetoothState::Streaming
                        | crate::services::radio::BluetoothState::Starting
                ),
                AppMsg::Bluetooth,
            )
            .clickable(!info.busy),
        ))
        .child(row(
            "bluetooth-state",
            "蓝牙状态",
            text(info.bluetooth.label(), font_20(), MUTED),
        ));
    if let Some(message) = session.message {
        body.child(note("wireless-message", message))
    } else {
        body
    }
}
