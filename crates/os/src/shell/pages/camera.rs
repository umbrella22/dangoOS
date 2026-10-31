//! Camera preview controls and connection status.
use alloc::format;

use rs_lvgl_ui::{Align, Key, Size, Style, View};

use crate::shell::messages::AppMsg;
use crate::shell::navigation::route::AppId;

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
pub fn camera(s: crate::services::camera::Snapshot) -> View<AppMsg> {
    let live = s.state == crate::services::camera::State::Preview;
    let status = if s.error.is_some() {
        "相机未就绪，请检查左侧插槽或重试".into()
    } else if live {
        format!("640×480 · 已保存 {}", s.saved)
    } else {
        "正在连接左侧相机…".into()
    };
    page(
        AppId::Camera.key(),
        "相机",
        rs_lvgl_ui::obj()
            .key(Key::Named("viewfinder"))
            .scrollable(false)
            .style(
                Style::neutral()
                    .bg(CARD)
                    .width(Size::Px(360))
                    .height(Size::Px(270))
                    .align(Align::TopMid)
                    .offset(0, 96),
            )
            .child(
                text(
                    if live {
                        ""
                    } else if s.error.is_some() {
                        "相机未就绪"
                    } else {
                        "正在启动"
                    },
                    font_24(),
                    MUTED,
                )
                .style(Style::default().align(Align::Center)),
            ),
    )
    .child(
        text(status, font_20(), MUTED)
            .key(Key::Named("capture-status"))
            .style(Style::default().align(Align::TopMid).offset(0, 68)),
    )
    .child(
        rs_lvgl_ui::button(if live { "拍照" } else { "重试" })
            .key(Key::Named("shutter"))
            .style(
                Style::neutral()
                    .width(Size::Px(144))
                    .height(Size::Px(44))
                    .bg(ACCENT)
                    .fg(TEXT)
                    .font(font_24())
                    .radius(22)
                    .align(Align::TopMid)
                    .offset(0, 372),
            )
            .on_click(if live {
                AppMsg::CameraCapture
            } else {
                AppMsg::CameraRetry
            }),
    )
}
