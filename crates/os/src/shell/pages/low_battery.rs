//! Terminal battery warning.

use rs_lvgl_ui::{Align, Key, Size, View};

use crate::shell::messages::AppMsg;

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
/// Terminal battery warning, independent of mounted apps, skins and Lua.
pub(crate) fn low_battery() -> View<AppMsg> {
    rs_lvgl_ui::container()
        .key(Key::Named("low-battery-root"))
        .scrollable(false)
        .background(BG)
        .width(Size::Percent(100))
        .height(Size::Percent(100))
        .child(
            rs_lvgl_ui::container()
                .width(Size::Px(100))
                .height(Size::Px(56))
                .align(Align::Center)
                .offset(-3, -76)
                .radius(9)
                .border(MUTED, 3)
                .child(
                    rs_lvgl_ui::container()
                        .width(Size::Px(8))
                        .height(Size::Px(22))
                        .background(ACCENT)
                        .radius(2)
                        .align(Align::CenterLeft)
                        .offset(9, 0),
                ),
        )
        .child(
            rs_lvgl_ui::container()
                .width(Size::Px(6))
                .height(Size::Px(20))
                .background(MUTED)
                .radius(2)
                .align(Align::Center)
                .offset(52, -76),
        )
        .child(
            text("低电量", font_30(), TEXT)
                .align(Align::Center)
                .offset(0, 2),
        )
        .child(
            text("即将关机，请连接电源", font_20(), MUTED)
                .align(Align::Center)
                .offset(0, 48),
        )
}
