//! Countdown page.
use alloc::format;

use rs_lvgl_ui::{Align, Color, FlexAlign, Key, Part, Size, Style, TextAlign, View};

use crate::shell::messages::AppMsg;
use {
    crate::services::timer::format_mm_ss, crate::services::timer::Phase,
    crate::services::timer::PRESETS_MIN,
};
use {crate::shell::model::Hub, crate::shell::navigation::route::AppId};

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
// ------------------------------------------------------------------- timer

fn chip(index: usize, minutes: u32, selected: bool, enabled: bool) -> View<AppMsg> {
    let mut node = rs_lvgl_ui::obj()
        .key(Key::Id(index as u64))
        .scrollable(false)
        .clickable(enabled)
        .style(
            Style::neutral()
                .bg(if selected { ACCENT } else { CARD })
                .radius(22)
                .width(Size::Px(66))
                .height(Size::Px(44))
                .opacity(if enabled || selected { 255 } else { 110 }),
        )
        .child(
            text(format!("{}", minutes), font_24(), TEXT)
                .key(Key::Named("label"))
                .style(Style::default().align(Align::Center)),
        );
    if enabled {
        node = node.on_click(AppMsg::TimerPreset(index));
    }
    node
}

fn pill_button(key: &'static str, label: &'static str, color: Color, msg: AppMsg) -> View<AppMsg> {
    rs_lvgl_ui::obj()
        .key(Key::Named(key))
        .scrollable(false)
        .clickable(true)
        .on_click(msg)
        .style(
            Style::neutral()
                .bg(color)
                .radius(28)
                .width(Size::Px(156))
                .height(Size::Px(56)),
        )
        .child(
            text(label, font_24(), TEXT)
                .key(Key::Named("label"))
                .style(Style::default().align(Align::Center)),
        )
}

/// Timer: ring, clock, presets, start/pause and reset.
pub fn timer(hub: &Hub) -> View<AppMsg> {
    let countdown = &hub.countdown;
    let now = hub.now_ms;
    let phase = countdown.phase();
    let clock = format_mm_ss(countdown.display_seconds(now));
    let (status, primary) = match phase {
        Phase::Ready => ("准备", "开始"),
        Phase::Running => ("专注中", "暂停"),
        Phase::Paused => ("已暂停", "继续"),
        Phase::Done => ("时间到", "再来"),
    };
    let editable = matches!(phase, Phase::Ready | Phase::Done);

    let permille = countdown.remaining_permille(now).min(1000) as i32;
    let ring = rs_lvgl_ui::obj()
        .key(Key::Named("ring"))
        .scrollable(false)
        .style(
            Style::neutral()
                .width(Size::Px(224))
                .height(Size::Px(224))
                .align(Align::TopMid)
                .offset(0, 80),
        )
        .child(
            rs_lvgl_ui::arc(0, 1000, permille)
                .key(Key::Named("arc"))
                .clickable(false)
                .style(
                    Style::default()
                        .width(Size::Percent(100))
                        .height(Size::Percent(100))
                        .arc_color(CARD_HI)
                        .arc_width(14),
                )
                .part_style(
                    Part::Indicator,
                    Style::default().arc_color(ACCENT).arc_width(14),
                )
                .part_style(Part::Knob, Style::default().bg_opacity(0).pad(0)),
        )
        .child(
            rs_lvgl_ui::column([])
                .key(Key::Named("face"))
                .scrollable(false)
                .style(
                    Style::neutral()
                        .width(Size::Content)
                        .height(Size::Content)
                        .align(Align::Center)
                        .flex_align(FlexAlign::Start, FlexAlign::Center),
                )
                .child(
                    text(clock.as_str(), font_digits(), TEXT)
                        .key(Key::Named("clock"))
                        .style(Style::default().text_align(TextAlign::Center)),
                )
                .child(text(status, font_20(), MUTED).key(Key::Named("status"))),
        );

    let chips = rs_lvgl_ui::row([])
        .key(Key::Named("presets"))
        .scrollable(false)
        .style(
            Style::neutral()
                .width(Size::Content)
                .height(Size::Content)
                .gap(10)
                .align(Align::TopMid)
                .offset(0, 312),
        )
        .children(
            PRESETS_MIN
                .iter()
                .enumerate()
                .map(|(i, &m)| chip(i, m, i == countdown.preset(), editable)),
        );

    let controls = rs_lvgl_ui::row([])
        .key(Key::Named("controls"))
        .scrollable(false)
        .style(
            Style::neutral()
                .width(Size::Content)
                .height(Size::Content)
                .gap(16)
                .align(Align::TopMid)
                .offset(0, 362),
        )
        .child(pill_button("primary", primary, ACCENT, AppMsg::TimerToggle))
        .child(pill_button("reset", "重置", CARD_HI, AppMsg::TimerReset));

    rs_lvgl_ui::obj()
        .key(Key::Named(AppId::Timer.key()))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(BG)
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            text(AppId::Timer.title(), font_30(), TITLE)
                .key(Key::Named("title"))
                .style(Style::default().align(Align::TopMid).offset(0, 30)),
        )
        .child(ring)
        .child(chips)
        .child(controls)
        .child(home_indicator())
}
