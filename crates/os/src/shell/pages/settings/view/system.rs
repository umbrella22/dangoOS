//! System settings page.
use super::components::{list, size};
use crate::shell::{
    components::page::{font_20, row, text},
    messages::AppMsg,
    model::Hub,
    pages::settings::model::Info,
    theme::tokens::MUTED,
};
use alloc::{format, string::String};
use rs_lvgl_ui::View;

pub(super) fn view(hub: &Hub, info: Info) -> View<AppMsg> {
    list("system-info")
        .child(row("model", "设备", text("ESP-Mosaico", font_20(), MUTED)))
        .child(row(
            "version",
            "固件版本",
            text(env!("CARGO_PKG_VERSION"), font_20(), MUTED),
        ))
        .child(row(
            "uptime",
            "运行时间",
            text(format!("{} 分", hub.about.uptime_s / 60), font_20(), MUTED),
        ))
        .child(row(
            "heap",
            "可用内存",
            text(
                hub.about
                    .free_kib
                    .map(size)
                    .unwrap_or_else(|| String::from("读取中")),
                font_20(),
                MUTED,
            ),
        ))
        .child(row(
            "ota",
            "固件状态",
            text(
                if info.candidate {
                    "启动检查中"
                } else {
                    "已确认"
                },
                font_20(),
                MUTED,
            ),
        ))
        .child(row(
            "slot",
            "启动槽",
            text(
                match info.boot_slot {
                    Some(0) => "A",
                    Some(1) => "B",
                    _ => "未知",
                },
                font_20(),
                MUTED,
            ),
        ))
}
