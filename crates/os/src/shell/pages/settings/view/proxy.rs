//! System-update proxy editor and its keyboard session.
use crate::shell::{
    components::page::{card, font_20, font_24, page, text},
    messages::AppMsg,
    navigation::route::AppId,
    pages::settings::model::Session,
    theme::tokens::*,
};
use rs_lvgl_ui::{Align, Key, Part, Size, State, Style, View};

pub(super) fn view(session: &Session) -> View<AppMsg> {
    page(
        AppId::Settings.key(),
        "更新代理",
        text("HTTP 无认证 · IPv4:端口", font_20(), MUTED).style(
            Style::default()
                .width(Size::Px(400))
                .align(Align::TopMid)
                .offset(0, 78),
        ),
    )
    .child(
        rs_lvgl_ui::textarea(&session.proxy_address)
            .key(Key::Named("proxy-endpoint"))
            .keyboard_id(702)
            .placeholder("例如 10.0.0.1:7890")
            .style(
                card()
                    .width(Size::Px(400))
                    .height(Size::Px(52))
                    .font(font_24())
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(40, 112),
            )
            .on_text_changed(|s| AppMsg::UpdateProxyText(s.into())),
    )
    .child(
        rs_lvgl_ui::button("取消")
            .key(Key::Named("proxy-cancel"))
            .style(
                card()
                    .width(Size::Px(188))
                    .height(Size::Px(44))
                    .font(font_24())
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(40, 177),
            )
            .on_click(AppMsg::SettingsBack),
    )
    .child(
        rs_lvgl_ui::button("保存并启用")
            .key(Key::Named("proxy-apply"))
            .style(
                card()
                    .width(Size::Px(188))
                    .height(Size::Px(44))
                    .font(font_24())
                    .bg(ACCENT)
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(252, 177),
            )
            .on_click(AppMsg::UpdateProxyApply),
    )
    .child(
        rs_lvgl_ui::keyboard(702)
            .key(Key::Named("proxy-keyboard"))
            .style(
                Style::default()
                    .width(Size::Px(400))
                    .height(Size::Px(184))
                    .bg(BG)
                    .align(Align::TopMid)
                    .offset(0, 238),
            )
            .part_style(
                Part::Items,
                Style::neutral()
                    .bg(CARD_HI)
                    .fg(TEXT)
                    .font(font_20())
                    .radius(8),
            )
            .part_state_style(Part::Items, State::PRESSED, Style::default().bg(ACCENT)),
    )
    .child(
        text(
            session
                .message
                .unwrap_or("HTTP / mixed 代理，仅用于系统更新"),
            font_20(),
            MUTED,
        )
        .key(Key::Named("proxy-error"))
        .style(
            Style::default()
                .width(Size::Px(400))
                .align(Align::TopMid)
                .offset(0, 428),
        ),
    )
}
