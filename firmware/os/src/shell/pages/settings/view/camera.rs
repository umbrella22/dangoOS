//! Camera settings page.
use super::components::{entry, list, note};
use crate::services::settings::preferences::Settings;
use crate::shell::{
    components::page::{row, switch},
    messages::AppMsg,
    navigation::route::AppId,
};
use rs_lvgl_ui::{Key, View};

pub(super) fn view(s: &Settings) -> View<AppMsg> {
    let mut body = list("camera-controls")
        .child(row(
            "mirror",
            "水平镜像",
            switch("switch", s.camera_mirror, AppMsg::CameraMirror),
        ))
        .child(note("quality-title", "照片质量"));
    for (index, quality) in [75, 85, 95].into_iter().enumerate() {
        body = body.child(entry(
            Key::Id(index as u64),
            match quality {
                75 => "节省空间",
                85 => "标准",
                _ => "高画质",
            },
            if quality == s.jpeg_quality {
                "已选"
            } else {
                ""
            },
            AppMsg::JpegQuality(quality),
        ));
    }
    body.child(entry(
        Key::Named("open-camera"),
        "打开相机",
        "›",
        AppMsg::Open(AppId::Camera),
    ))
}
