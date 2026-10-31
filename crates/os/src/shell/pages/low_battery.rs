//! Terminal battery warning.

use rs_lvgl_ui::{Align, Key, Size, Style, View};

use crate::shell::messages::AppMsg;

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
/// Terminal battery warning, independent of mounted apps, skins and Lua.
pub(crate) fn low_battery() -> View<AppMsg> {
    rs_lvgl_ui::obj()
        .key(Key::Named("low-battery-root"))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(BG)
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            rs_lvgl_ui::obj()
                .style(
                    Style::neutral()
                        .width(Size::Px(100))
                        .height(Size::Px(56))
                        .align(Align::Center)
                        .offset(-3, -76)
                        .radius(9)
                        .border(MUTED, 3),
                )
                .child(
                    rs_lvgl_ui::obj().style(
                        Style::neutral()
                            .width(Size::Px(8))
                            .height(Size::Px(22))
                            .bg(ACCENT)
                            .radius(2)
                            .align(Align::LeftMid)
                            .offset(9, 0),
                    ),
                ),
        )
        .child(
            rs_lvgl_ui::obj().style(
                Style::neutral()
                    .width(Size::Px(6))
                    .height(Size::Px(20))
                    .bg(MUTED)
                    .radius(2)
                    .align(Align::Center)
                    .offset(52, -76),
            ),
        )
        .child(
            text("低电量", font_30(), TEXT)
                .style(Style::default().align(Align::Center).offset(0, 2)),
        )
        .child(
            text("即将关机，请连接电源", font_20(), MUTED)
                .style(Style::default().align(Align::Center).offset(0, 48)),
        )
}
