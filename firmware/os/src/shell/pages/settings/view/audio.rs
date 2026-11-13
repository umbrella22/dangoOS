//! Audio settings page.
use super::components::{level_slider, list};
use crate::services::settings::preferences::Settings;
use crate::shell::{
    components::page::{font_20, row, switch, text},
    messages::AppMsg,
    pages::settings::model::Info,
    theme::tokens::MUTED,
};
use rs_lvgl_ui::View;

pub(super) fn view(s: &Settings, info: Info) -> View<AppMsg> {
    list("audio-controls")
        .child(level_slider(
            "volume",
            "音量",
            0,
            s.audio_volume,
            AppMsg::AudioVolume,
        ))
        .child(row(
            "audio-mute",
            "静音",
            switch("switch", s.audio_muted, AppMsg::AudioMute),
        ))
        .child(row(
            "audio-state",
            "音频状态",
            text(
                match info.audio {
                    crate::services::audio::State::Off => "已停止",
                    crate::services::audio::State::Starting => "正在启动",
                    crate::services::audio::State::Ready => "已就绪",
                    crate::services::audio::State::Playing => "正在播放",
                    crate::services::audio::State::Recording => "正在录音",
                    crate::services::audio::State::Fault => "音频故障",
                },
                font_20(),
                MUTED,
            ),
        ))
}
