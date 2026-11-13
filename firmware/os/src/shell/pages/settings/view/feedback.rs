//! Feedback settings page.
use super::components::list;
use crate::services::settings::preferences::Settings;
use crate::shell::{
    components::page::{row, switch},
    messages::AppMsg,
};
use rs_lvgl_ui::View;

pub(super) fn view(s: &Settings) -> View<AppMsg> {
    list("feedback-controls")
        .child(row(
            "lights",
            "灯光",
            switch("switch", s.lights, AppMsg::Lights),
        ))
        .child(row(
            "haptics",
            "震动",
            switch("switch", s.haptics, AppMsg::Haptics),
        ))
}
