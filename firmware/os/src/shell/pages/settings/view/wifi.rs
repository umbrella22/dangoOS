//! Wifi settings page.
use super::components::{entry, list, note};
use crate::shell::{
    components::page::{row, switch},
    messages::AppMsg,
    pages::settings::model::{ssid_label, Session},
};
use alloc::format;
use rs_lvgl_ui::{Key, View};

pub(super) fn view(session: &Session) -> View<AppMsg> {
    let info = session.info;
    let state = match info.ip {
        Some(b) => format!(
            "{} · {}.{}.{}.{}",
            info.wifi.label(),
            b[0],
            b[1],
            b[2],
            b[3]
        ),
        None => info.wifi.label().into(),
    };
    let mut body = list("wifi-networks")
        .child(row(
            "wifi-enabled",
            "Wi-Fi",
            switch("switch", info.wifi.enabled(), AppMsg::Wifi).clickable(!info.busy),
        ))
        .child(note("wifi-state", state));
    if let Some(ssid) = &session.wifi_saved.current {
        body = body.child(note("wifi-current", format!("当前：{}", ssid_label(ssid))));
    }
    if session.wifi_saved.loaded {
        body = body.child(row(
            "wifi-automatic",
            "自动连接",
            switch(
                "switch",
                session.wifi_saved.automatic,
                AppMsg::WifiAutomatic,
            )
            .clickable(!info.busy),
        ));
    }
    if session.wifi_saved.error {
        body = body.child(note("wifi-save-error", "网络未保存，请重试"));
    }
    if !session.wifi_saved.networks.is_empty() {
        body = body.child(note("wifi-known-title", "已保存网络"));
        for (index, ssid) in session.wifi_saved.networks.iter().enumerate() {
            body = body.child(
                entry(
                    Key::Id(1000 + index as u64),
                    ssid_label(ssid),
                    if session.wifi_saved.current.as_ref() == Some(ssid) {
                        "已连接"
                    } else {
                        "›"
                    },
                    AppMsg::WifiChoose(crate::services::radio::Network {
                        ssid: ssid.clone(),
                        channel: 0,
                        rssi: 0,
                        auth: "WPA2".into(),
                    }),
                )
                .clickable(!info.busy),
            );
        }
    }
    if info.wifi.enabled() {
        body = body.child(
            entry(
                Key::Named("scan"),
                if info.busy {
                    "操作进行中"
                } else {
                    "搜索附近网络"
                },
                "›",
                AppMsg::WifiScan,
            )
            .clickable(!info.busy),
        );
    }
    if let Some(message) = session.message {
        body = body.child(note("wifi-message", message));
    }
    for (index, network) in session.networks.iter().enumerate().filter(|(_, network)| {
        info.wifi.enabled() && !session.wifi_saved.networks.contains(&network.ssid)
    }) {
        let name = ssid_label(&network.ssid);
        body = body.child(
            entry(
                Key::Id(index as u64),
                name,
                format!("{} dBm", network.rssi),
                AppMsg::WifiChoose(network.clone()),
            )
            .clickable(!info.busy && !network.ssid.is_empty()),
        );
    }
    body
}

pub(super) fn known_network(session: &Session) -> View<AppMsg> {
    let info = session.info;
    let name = session
        .selected
        .as_ref()
        .map(|n| ssid_label(&n.ssid))
        .unwrap_or_default();
    list("wifi-known-network")
        .child(note("wifi-known-name", name))
        .child(
            entry(
                Key::Named("wifi-known-connect"),
                "连接此网络",
                "›",
                AppMsg::WifiSavedConnect,
            )
            .clickable(!info.busy),
        )
        .child(
            entry(
                Key::Named("wifi-known-key"),
                "重新输入密码",
                "›",
                AppMsg::WifiReenterPassword,
            )
            .clickable(!info.busy),
        )
        .child(
            entry(
                Key::Named("wifi-known-forget"),
                "忘记此网络",
                "›",
                AppMsg::WifiForget,
            )
            .clickable(!info.busy),
        )
}
