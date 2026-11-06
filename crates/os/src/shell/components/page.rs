//! Native page frames, controls and font-aware text fitting.
use alloc::string::String;

use rs_lvgl_ui::{
    Align, Color, FlexAlign, Font, ItemAlign, Key, NativePart, Size, State, Style, View,
};

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
    rs_lvgl_ui::label(content).font(font).text_color(color)
}

pub(crate) fn text_static(content: &'static str, font: Font, color: Color) -> View<AppMsg> {
    rs_lvgl_ui::label_static(content)
        .font(font)
        .text_color(color)
}

/// A full-screen page: black field, centred title, home indicator.
pub(crate) fn page(key: &'static str, title: &'static str, body: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::container()
        .key(Key::Named(key))
        .scrollable(false)
        .background(BG)
        .width(Size::Percent(100))
        .height(Size::Percent(100))
        .child(
            text_static(title, font_30(), TITLE)
                .key(Key::Named("title"))
                .align(Align::TopCenter)
                .offset(0, 30),
        )
        .child(body)
        .child(home_indicator())
}

/// The pill at the bottom edge: "swipe up from here".
pub(crate) fn home_indicator() -> View<AppMsg> {
    rs_lvgl_ui::container()
        .key(Key::Named("home-pill"))
        .background(WEAK)
        .radius(3)
        .width(Size::Px(96))
        .height(Size::Px(6))
        .align(Align::BottomCenter)
        .offset(0, -16)
}

pub(crate) fn card() -> Style {
    Style::plain().background(CARD).radius(RADIUS).padding(20)
}

/// A 68 px row: label on the left, `trailing` on the right.
pub(crate) fn row(key: &'static str, label: &'static str, trailing: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::row([])
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            card()
                .padding_top(0)
                .padding_bottom(0)
                .width(Size::Percent(100))
                .height(Size::Px(ROW_H))
                .justify_content(FlexAlign::SpaceBetween)
                .align_items(ItemAlign::Center)
                .align_content(FlexAlign::Center),
        )
        .child(text_static(label, font_24(), TEXT).key(Key::Named("label")))
        .child(trailing)
}

pub(crate) fn switch(key: &'static str, on: bool, msg: fn(bool) -> AppMsg) -> View<AppMsg> {
    rs_lvgl_ui::switch(on)
        .key(Key::Named(key))
        .width(Size::Px(64))
        .height(Size::Px(34))
        .background(CARD_HI)
        .native_part_state_style(
            NativePart::Indicator,
            State::CHECKED,
            Style::default().background(ACCENT),
        )
        .thumb_style(Style::default().background(TEXT))
        .on_toggle(msg)
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
