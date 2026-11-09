//! A scrollable reading window over a NAND conversation. Its model is copied from the service;
//! buttons enqueue commands and never perform capture or network IO here.
use crate::{
    services::voice::{Phase, Snapshot},
    shell::{components::page::*, messages::AppMsg, theme::tokens::*},
};
use alloc::format;
use rs_lvgl_ui::{Align, FlexAlign, ItemAlign, Key, Size, Style, TextAlign, View};

fn button(key: &'static str, label: &'static str, msg: AppMsg, primary: bool) -> View<AppMsg> {
    rs_lvgl_ui::button(label)
        .key(Key::Named(key))
        .scrollable(false)
        .on_click(msg)
        .style(
            Style::plain()
                .background(if primary { ACCENT } else { CARD_HI })
                .radius(25)
                .width(Size::Px(156))
                .height(Size::Px(50))
                .font(crate::shell::theme::font::VOICE_20.font())
                .text_color(TEXT),
        )
}
// Both speakers use the same text node; share its construction and cleanup.
#[inline(never)]
fn utterance(key: &'static str, prefix: &'static str, content: &str) -> View<AppMsg> {
    text(
        format!("{prefix}{content}"),
        crate::shell::theme::font::VOICE_20.font(),
        TEXT,
    )
    .key(Key::Named(key))
    .width(Size::Percent(100))
}
pub fn view(model: &Snapshot) -> View<AppMsg> {
    let status = match model.phase {
        Phase::Idle => "按 AI 键开始",
        Phase::Preparing => "正在准备…",
        Phase::Listening => "正在录音，再按 AI 键发送",
        Phase::Transcribing => "正在识别…",
        Phase::Replying => "正在回复…",
        Phase::Done => "回复完成",
        Phase::Cancelled => "已取消",
        Phase::Error => "本次对话未完成",
    };
    let (primary, action) = match model.phase {
        Phase::Listening => ("结束并发送", AppMsg::VoiceToggle),
        Phase::Preparing | Phase::Transcribing | Phase::Replying => ("取消", AppMsg::VoiceCancel),
        _ => ("开始录音", AppMsg::VoiceToggle),
    };
    let paging = !model.phase.active() && model.reply_bytes > model.reply.len();
    let mut conversation = rs_lvgl_ui::column([])
        .key(Key::Named("conversation"))
        .width(Size::Px(392))
        .height(Size::Px(if paging { 212 } else { 244 }))
        .align(Align::TopCenter)
        .offset(0, 120)
        .padding(12)
        .gap(12)
        .align_items(ItemAlign::Start)
        .align_content(FlexAlign::Start);
    if model.transcript.is_empty() && model.reply.is_empty() {
        conversation = conversation.child(
            text_static(
                "说完后再按一次 AI 键发送。\n双击取消，离开页面也会取消。\n每次最多录音 30 秒。",
                font_20(),
                MUTED,
            )
            .key(Key::Named("hint"))
            .width(Size::Percent(100)),
        );
    }
    if !model.transcript.is_empty() {
        conversation = conversation.child(utterance("transcript", "你：", &model.transcript));
    }
    if !model.reply.is_empty() {
        conversation = conversation.child(utterance("reply", "团子：", &model.reply));
    }
    if let Some(error) = model.error {
        conversation = conversation.child(
            text_static(error, crate::shell::theme::font::VOICE_20.font(), MUTED)
                .key(Key::Named("error"))
                .width(Size::Percent(100)),
        );
    }
    let mut controls = rs_lvgl_ui::row([])
        .key(Key::Named("controls"))
        .scrollable(false)
        .width(Size::Content)
        .height(Size::Content)
        .align(Align::TopCenter)
        .offset(0, if paging { 394 } else { 378 })
        .gap(12)
        .child(button("primary", primary, action, true));
    if model.phase == Phase::Listening {
        controls = controls.child(button("cancel", "取消", AppMsg::VoiceCancel, false));
    }
    let mut body = rs_lvgl_ui::container()
        .scrollable(false)
        .width(Size::Percent(100))
        .height(Size::Percent(100))
        .child(
            text(
                format!(
                    "{}  {}.{}s",
                    status,
                    model.recorded_ms / 1000,
                    model.recorded_ms % 1000 / 100
                ),
                font_20(),
                MUTED,
            )
            .key(Key::Named("status"))
            .width(Size::Px(400))
            .text_align(TextAlign::Center)
            .align(Align::TopCenter)
            .offset(0, 84),
        )
        .child(conversation)
        .child(controls);
    if paging {
        let mut pages = rs_lvgl_ui::row([])
            .key(Key::Named("reply-pages"))
            .width(Size::Content)
            .height(Size::Content)
            .align(Align::TopCenter)
            .offset(0, 346)
            .gap(20);
        if model.reply_start > 0 {
            pages = pages.child(
                button("previous", "‹", AppMsg::VoicePage(false), false)
                    .width(Size::Px(96))
                    .height(Size::Px(36)),
            );
        }
        if model.reply_start + model.reply.len() < model.reply_bytes {
            pages = pages.child(
                button("next", "›", AppMsg::VoicePage(true), false)
                    .width(Size::Px(96))
                    .height(Size::Px(36)),
            );
        }
        body = body.child(pages);
    }
    page("app-voice", "语音", body)
}
