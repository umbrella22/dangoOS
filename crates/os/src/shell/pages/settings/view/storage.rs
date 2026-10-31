//! Storage settings page.
use super::components::{list, size};
use crate::shell::{
    components::page::{font_20, row, text},
    messages::AppMsg,
    pages::settings::model::Info,
    theme::tokens::MUTED,
};
use rs_lvgl_ui::View;

pub(super) fn view(info: Info) -> View<AppMsg> {
    list("storage-info")
        .child(row(
            "volume",
            "数据存储",
            text(
                if info.storage_ready {
                    "已挂载"
                } else {
                    "未就绪"
                },
                font_20(),
                MUTED,
            ),
        ))
        .child(row(
            "total",
            "总空间",
            text(size(info.data_total_kib), font_20(), MUTED),
        ))
        .child(row(
            "free",
            "可用空间",
            text(size(info.data_free_kib), font_20(), MUTED),
        ))
}
