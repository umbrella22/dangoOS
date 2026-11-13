//! Display settings page.
use super::components::{entry, level_slider, list};
use crate::services::settings::preferences::Settings;
use crate::shell::{
    components::page::{font_20, row, text},
    messages::AppMsg,
    theme::tokens::MUTED,
};
use rs_lvgl_ui::{Key, View};

pub(super) fn view(s: &Settings) -> View<AppMsg> {
    list("display-controls")
        .child(level_slider(
            "brightness",
            "亮度",
            10,
            s.brightness,
            AppMsg::Brightness,
        ))
        .child(entry(
            Key::Named("screen-off"),
            "熄屏 30 秒",
            "轻触可唤醒",
            AppMsg::ScreenOff,
        ))
        .child(row(
            "deep-sleep",
            "深度睡眠",
            text("暂未启用", font_20(), MUTED),
        ))
}
