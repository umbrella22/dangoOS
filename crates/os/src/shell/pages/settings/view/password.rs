//! Wi-Fi password editor and its keyboard session.
use crate::shell::{
    components::page::{card, fit_text, font_20, font_24, page, text},
    messages::AppMsg,
    navigation::route::AppId,
    pages::settings::model::{ssid_label, Password, Session},
    theme::tokens::*,
};
use rs_lvgl_ui::{Align, Key, Part, Size, State, Style, View};

pub(super) fn view(session: &Session) -> View<AppMsg> {
    let name = session
        .selected
        .as_ref()
        .map(|n| ssid_label(&n.ssid))
        .unwrap_or_default();
    page(
        AppId::Settings.key(),
        "连接网络",
        text(fit_text(&name, 400), font_20(), MUTED)
            .key(Key::Named("ssid"))
            .style(
                Style::default()
                    .width(Size::Px(400))
                    .align(Align::TopMid)
                    .offset(0, 78),
            ),
    )
    .child(
        rs_lvgl_ui::textarea(&session.password.0)
            .key(Key::Named("wifi-password"))
            .keyboard_id(701)
            .password(session.password_hidden)
            .placeholder("输入 Wi-Fi 密码")
            .style(
                card()
                    .width(Size::Px(304))
                    .height(Size::Px(52))
                    .font(font_24())
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(40, 112),
            )
            .on_text_changed(|s| AppMsg::WifiPassword(Password(s.into()))),
    )
    .child(
        rs_lvgl_ui::button(if session.password_hidden {
            "显示"
        } else {
            "隐藏"
        })
        .key(Key::Named("password-visibility"))
        .style(
            card()
                .width(Size::Px(88))
                .height(Size::Px(52))
                .font(font_20())
                .fg(TEXT)
                .shadow_width(0)
                .align(Align::TopLeft)
                .offset(352, 112),
        )
        .on_click(AppMsg::WifiPasswordHidden(!session.password_hidden)),
    )
    .child(
        rs_lvgl_ui::button("取消")
            .key(Key::Named("cancel"))
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
        rs_lvgl_ui::button("连接")
            .key(Key::Named("connect"))
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
            .on_click(AppMsg::WifiConnect),
    )
    .child(
        rs_lvgl_ui::keyboard(701)
            .key(Key::Named("wifi-keyboard"))
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
        text(session.message.unwrap_or(""), font_20(), MUTED)
            .key(Key::Named("password-error"))
            .style(Style::default().align(Align::TopMid).offset(0, 428)),
    )
}
