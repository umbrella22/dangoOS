//! Native page frames, controls and font-aware text fitting.
use alloc::string::String;

use rs_lvgl_ui::{Align, Color, FlexAlign, Font, Key, Part, Size, State, Style, View};

use crate::shell::messages::AppMsg;
use {
    crate::shell::theme::font::HUB_20, crate::shell::theme::font::HUB_24,
    crate::shell::theme::font::HUB_30, crate::shell::theme::font::HUB_DIGITS_64,
};

use crate::shell::theme::tokens::*;
pub(crate) fn font_20() -> Font {
    HUB_20.font()
}
pub(crate) fn font_24() -> Font {
    HUB_24.font()
}
pub(crate) fn font_30() -> Font {
    HUB_30.font()
}
pub(crate) fn font_digits() -> Font {
    HUB_DIGITS_64.font()
}
pub(crate) fn font_icon() -> Font {
    // SAFETY: a static LVGL font enabled by the vendored configuration.
    unsafe { Font::from_raw(core::ptr::addr_of!(rs_lvgl::sys::lv_font_montserrat_48)) }
}

pub(crate) fn text(content: impl Into<String>, font: Font, color: Color) -> View<AppMsg> {
    rs_lvgl_ui::label(content).style(Style::neutral().font(font).fg(color))
}

/// A full-screen page: black field, centred title, home indicator.
pub(crate) fn page(key: &'static str, title: &'static str, body: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::obj()
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(BG)
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            text(title, font_30(), TITLE)
                .key(Key::Named("title"))
                .style(Style::default().align(Align::TopMid).offset(0, 30)),
        )
        .child(body)
        .child(home_indicator())
}

/// The pill at the bottom edge: "swipe up from here".
pub(crate) fn home_indicator() -> View<AppMsg> {
    rs_lvgl_ui::obj().key(Key::Named("home-pill")).style(
        Style::neutral()
            .bg(WEAK)
            .radius(3)
            .width(Size::Px(96))
            .height(Size::Px(6))
            .align(Align::BottomMid)
            .offset(0, -16),
    )
}

pub(crate) fn card() -> Style {
    Style::neutral().bg(CARD).radius(RADIUS).pad(20)
}

/// A 68 px row: label on the left, `trailing` on the right.
pub(crate) fn row(key: &'static str, label: &'static str, trailing: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::row([])
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            card()
                .pad_top(0)
                .pad_bottom(0)
                .width(Size::Percent(100))
                .height(Size::Px(ROW_H))
                .flex_align(FlexAlign::SpaceBetween, FlexAlign::Center),
        )
        .child(text(label, font_24(), TEXT).key(Key::Named("label")))
        .child(trailing)
}

pub(crate) fn switch(key: &'static str, on: bool, msg: fn(bool) -> AppMsg) -> View<AppMsg> {
    rs_lvgl_ui::switch(on)
        .key(Key::Named(key))
        .style(
            Style::default()
                .width(Size::Px(64))
                .height(Size::Px(34))
                .bg(CARD_HI),
        )
        .part_state_style(Part::Indicator, State::CHECKED, Style::default().bg(ACCENT))
        .part_style(Part::Knob, Style::default().bg(TEXT))
        .on_value_changed(move |v| msg(v != 0))
}

pub(crate) fn fit_text(name: &str, max: u32) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = clean.as_str();
    let w = |c: char| crate::shell::theme::font::advance(&HUB_24, c).unwrap_or(24) as u32;
    if name.chars().map(w).sum::<u32>() <= max {
        return name.into();
    }
    let mut out = String::new();
    let mut used = w('…');
    for c in name.chars() {
        if used + w(c) > max {
            break;
        }
        used += w(c);
        out.push(c);
    }
    out.push('…');
    out
}
