//! Categories settings page.
use super::components::{entry, list, size};
use crate::shell::{
    messages::AppMsg,
    model::Hub,
    pages::settings::model::{Page, Session},
};
use alloc::format;
use rs_lvgl_ui::{Key, View};

pub(super) fn view(hub: &Hub, session: &Session) -> View<AppMsg> {
    let s = &hub.settings;
    let info = session.info;
    list("categories")
        .child(entry(
            Key::Named("character"),
            "更换团子",
            format!(
                "{}  ›",
                session
                    .skins
                    .iter()
                    .find(|choice| choice.id == session.selected_skin)
                    .map(|choice| choice.name.as_str())
                    .unwrap_or("读取中")
            ),
            AppMsg::SettingsPage(Page::Character),
        ))
        .child(entry(
            Key::Named("display"),
            "显示与电源",
            format!("{}%  ›", s.brightness),
            AppMsg::SettingsPage(Page::Display),
        ))
        .child(entry(
            Key::Named("audio"),
            "声音",
            format!("{}%  ›", s.audio_volume),
            AppMsg::SettingsPage(Page::Audio),
        ))
        .child(entry(
            Key::Named("feedback"),
            "灯光与震动",
            "›",
            AppMsg::SettingsPage(Page::Feedback),
        ))
        .child(entry(
            Key::Named("wireless"),
            "无线连接",
            info.wifi.label(),
            AppMsg::SettingsPage(Page::Wireless),
        ))
        .child(entry(
            Key::Named("camera"),
            "相机",
            "›",
            AppMsg::SettingsPage(Page::Camera),
        ))
        .child(entry(
            Key::Named("storage"),
            "存储",
            if info.storage_ready {
                size(info.data_free_kib)
            } else {
                "读取中".into()
            },
            AppMsg::SettingsPage(Page::Storage),
        ))
        .child(entry(
            Key::Named("update"),
            "系统更新",
            if session.update.phase == crate::services::update::Phase::Available {
                "有新版本"
            } else {
                "›"
            },
            AppMsg::SettingsPage(Page::Update),
        ))
        .child(entry(
            Key::Named("system"),
            "关于设备",
            "›",
            AppMsg::SettingsPage(Page::System),
        ))
}
