//! Firmware update status, actions and proxy settings.
use super::components::{entry, list, note};
use crate::shell::{
    components::page::{font_20, row, switch, text},
    messages::AppMsg,
    model::Hub,
    pages::settings::model::{Page, Session},
    theme::tokens::MUTED,
};
use alloc::format;
use rs_lvgl_ui::{Key, View};

pub(super) fn view(hub: &Hub, session: &Session) -> View<AppMsg> {
    use {crate::services::update::Command, crate::services::update::Phase};
    let update = &session.update;
    let busy = update.phase.busy();
    let mut body = list("update-controls")
        .child(row(
            "current-version",
            "当前版本",
            text(env!("CARGO_PKG_VERSION"), font_20(), MUTED),
        ))
        .child(row(
            "auto-check",
            "自动检查",
            switch(
                "switch",
                hub.settings.auto_update_check,
                AppMsg::AutoUpdateCheck,
            ),
        ))
        .child(note("update-source", "更新源：GitHub Releases"))
        .child(note("update-state", update.phase.label()));
    if let Some(version) = &update.version {
        body = body.child(row(
            "new-version",
            "发布版本",
            text(version, font_20(), MUTED),
        ));
    }
    if matches!(
        update.phase,
        Phase::Downloading | Phase::Installing | Phase::Verifying
    ) && update.total > 0
    {
        let percent = u64::from(update.received) * 100 / u64::from(update.total);
        body = body.child(note(
            "update-progress",
            format!(
                "{}% · {} / {} KB",
                percent,
                update.received / 1024,
                update.total / 1024
            ),
        ));
    }
    if let Some(error) = update.error {
        body = body.child(note("update-error", error));
    }
    if busy {
        if !matches!(update.phase, Phase::Installing | Phase::Restarting) {
            body = body.child(entry(
                Key::Named("cancel-update"),
                "取消更新",
                "",
                AppMsg::Update(Command::Cancel),
            ));
        }
    } else {
        body = body.child(
            entry(
                Key::Named("check-update"),
                "检查更新",
                "›",
                AppMsg::Update(Command::Check),
            )
            .clickable(!session.info.busy),
        );
        if update.phase == Phase::Available {
            body = body.child(
                entry(
                    Key::Named("install-update"),
                    "下载并安装",
                    "›",
                    AppMsg::Update(Command::Install),
                )
                .clickable(!session.info.busy),
            );
        }
    }
    body = body
        .child(row(
            "proxy-enabled",
            "手动代理",
            switch(
                "switch",
                hub.settings.update_proxy_enabled,
                AppMsg::UpdateProxyToggle,
            )
            .clickable(!busy && !session.info.busy),
        ))
        .child(
            entry(
                Key::Named("proxy-address"),
                "代理地址",
                hub.settings
                    .update_proxy
                    .map(crate::services::update::HttpProxy::address)
                    .unwrap_or_else(|| "未配置  ›".into()),
                AppMsg::SettingsPage(Page::Proxy),
            )
            .clickable(!busy && !session.info.busy),
        );
    body
}
