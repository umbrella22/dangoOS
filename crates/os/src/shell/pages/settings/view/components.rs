//! Shared settings controls, page frame and save feedback.
use crate::shell::{
    components::page::{card, fit_text, font_20, font_24, page, text},
    messages::AppMsg,
    navigation::route::AppId,
    pages::settings::model::{Info, Page},
    theme::tokens::*,
};
use alloc::{format, string::String};
use rs_lvgl_ui::{Align, FlexAlign, Key, Part, ScrollDir, ScrollbarMode, Size, Style, View};

pub(super) fn list(key: &'static str) -> View<AppMsg> {
    rs_lvgl_ui::column([])
        .key(Key::Named(key))
        .scrollable(true)
        .scroll_dir(ScrollDir::VER)
        .scrollbar_mode(ScrollbarMode::Auto)
        .style(
            Style::neutral()
                .width(Size::Px(400))
                .height(Size::Px(320))
                .gap(8)
                .align(Align::TopMid)
                .offset(0, 92)
                .pad_bottom(12),
        )
}
pub(super) fn entry(
    key: Key,
    label: impl Into<String>,
    value: impl Into<String>,
    message: AppMsg,
) -> View<AppMsg> {
    rs_lvgl_ui::obj()
        .key(key)
        .scrollable(false)
        .clickable(true)
        .style(
            card()
                .width(Size::Percent(100))
                .height(Size::Px(64))
                .pad_top(0)
                .pad_bottom(0),
        )
        .child(
            text(fit_text(&label.into(), 208), font_24(), TEXT)
                .key(Key::Named("label"))
                .style(Style::default().width(Size::Px(208)).align(Align::LeftMid)),
        )
        .child(
            text(value, font_20(), MUTED)
                .key(Key::Named("value"))
                .style(Style::default().align(Align::RightMid)),
        )
        .on_click(message)
}
pub(super) fn note(key: &'static str, value: impl Into<String>) -> View<AppMsg> {
    text(value, font_20(), MUTED)
        .key(Key::Named(key))
        .style(Style::default().width(Size::Percent(100)).pad(12))
}
pub(super) fn level_slider(
    key: &'static str,
    label: &'static str,
    min: i32,
    value: u8,
    changed: fn(i32) -> AppMsg,
) -> View<AppMsg> {
    rs_lvgl_ui::column([])
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            card()
                .width(Size::Percent(100))
                .height(Size::Content)
                .gap(18),
        )
        .child(
            rs_lvgl_ui::row([])
                .key(Key::Named("head"))
                .scrollable(false)
                .style(
                    Style::neutral()
                        .width(Size::Percent(100))
                        .height(Size::Content)
                        .flex_align(FlexAlign::SpaceBetween, FlexAlign::Center),
                )
                .child(text(label, font_24(), TEXT))
                .child(text(format!("{value}%"), font_24(), MUTED)),
        )
        .child(
            rs_lvgl_ui::slider(min, 100, i32::from(value))
                .key(Key::Named("slider"))
                .style(
                    Style::default()
                        .width(Size::Px(344))
                        .height(Size::Px(10))
                        .offset(8, 0)
                        .bg(CARD_HI)
                        .radius(5),
                )
                .part_style(Part::Indicator, Style::default().bg(ACCENT).radius(5))
                .part_style(Part::Knob, Style::default().bg(TEXT).pad(8))
                .on_value_changed(changed),
        )
}
pub(super) fn size(kib: u32) -> String {
    format!("{}.{:01} MB", kib / 1024, kib % 1024 * 10 / 1024)
}

pub(super) fn loading_spinner(key: &'static str) -> View<AppMsg> {
    use rs_lvgl::sys;
    // rs_lvgl_ui 0.2 applies arc styles only to Arc nodes. Configure the native
    // spinner directly until its Spinner node supports those part properties.
    // SAFETY: the factory returns a direct child; LVGL owns its animation and
    // the reconciler deletes the object when the saving feedback is removed.
    unsafe {
        rs_lvgl_ui::raw(|parent| {
            let object = core::ptr::NonNull::new(sys::lv_spinner_create(parent.as_ptr()))?;
            let p = object.as_ptr();
            sys::lv_obj_set_size(p, 20, 20);
            sys::lv_spinner_set_anim_params(p, 850, 200);
            sys::lv_obj_set_style_arc_color(
                p,
                sys::lv_color_make(CARD_HI.r, CARD_HI.g, CARD_HI.b),
                0,
            );
            sys::lv_obj_set_style_arc_width(p, 2, 0);
            sys::lv_obj_set_style_arc_color(
                p,
                sys::lv_color_make(ACCENT.r, ACCENT.g, ACCENT.b),
                sys::lv_part_t_LV_PART_INDICATOR,
            );
            sys::lv_obj_set_style_arc_width(p, 2, sys::lv_part_t_LV_PART_INDICATOR);
            Some(object)
        })
    }
    .key(Key::Named(key))
    .clickable(false)
    .style(Style::neutral().width(Size::Px(20)).height(Size::Px(20)))
}

pub(super) fn checkmark(key: &'static str, color: rs_lvgl_ui::Color) -> View<AppMsg> {
    // Reuse the icon font already embedded for the launcher. The outer box
    // keeps its layout at 24 px while LVGL scales the glyph around its centre.
    let font = unsafe {
        rs_lvgl_ui::Font::from_raw(core::ptr::addr_of!(rs_lvgl::sys::lv_font_montserrat_48))
    };
    rs_lvgl_ui::obj()
        .key(Key::Named(key))
        .scrollable(false)
        .clickable(false)
        .style(Style::neutral().width(Size::Px(24)).height(Size::Px(24)))
        .child(
            text(rs_lvgl_ui::symbol::OK, font, color).style(
                Style::default()
                    .align(Align::Center)
                    .transform_scale_x(128)
                    .transform_scale_y(128)
                    .transform_pivot_x(24)
                    .transform_pivot_y(24),
            ),
        )
}

pub(super) fn frame(body: View<AppMsg>, settings_page: Page, info: Info) -> View<AppMsg> {
    let mut view = page(AppId::Settings.key(), settings_page.title(), body);
    if settings_page != Page::Root {
        view = view.child(
            rs_lvgl_ui::button("‹")
                .key(Key::Named("settings-back"))
                .style(
                    Style::neutral()
                        .width(Size::Px(52))
                        .height(Size::Px(52))
                        .bg(BG)
                        .border(BG, 0)
                        .shadow_width(0)
                        .outline_width(0)
                        .fg(TEXT)
                        .font(font_24())
                        .align(Align::TopLeft)
                        .offset(34, 23),
                )
                .on_click(AppMsg::SettingsBack),
        );
    }
    if settings_page == Page::Character && !info.saving && !info.save_error {
        return view;
    }
    let mut feedback = rs_lvgl_ui::row([])
        .key(Key::Named("save-feedback"))
        .scrollable(false)
        .style(
            Style::neutral()
                .width(Size::Content)
                .height(Size::Px(28))
                .gap(8)
                .flex_align(FlexAlign::Center, FlexAlign::Center)
                .align(Align::TopMid)
                .offset(0, 430),
        );
    if info.save_error {
        feedback = feedback.child(text("保存失败，请重新调整", font_20(), YELLOW));
    } else if info.saving {
        feedback = feedback.child(loading_spinner("save-spinner"));
    } else if info.saved {
        feedback = feedback.child(checkmark("save-success", GREEN));
    }
    view.child(feedback)
}
