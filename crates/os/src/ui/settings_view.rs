//! Settings navigation, forms and status. Device operations run in tasks.
use super::{
    AppMsg,
    hub_view::{card, fit_text, font_20, font_24, page, row, switch, text, tokens::*},
};
use crate::hub::{
    AppId, Hub,
    settings::{Page, Password, Session, ssid_label},
};
use alloc::{format, string::String};
use rs_lvgl_ui::{Align, FlexAlign, Key, Part, ScrollDir, ScrollbarMode, Size, State, Style, View};

fn list(key: &'static str) -> View<AppMsg> {
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
fn entry(
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
fn note(key: &'static str, value: impl Into<String>) -> View<AppMsg> {
    text(value, font_20(), MUTED)
        .key(Key::Named(key))
        .style(Style::default().width(Size::Percent(100)).pad(12))
}
fn level_slider(key: &'static str,label: &'static str,min:i32,value:u8,changed:fn(i32)->AppMsg) -> View<AppMsg> {
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
fn size(kib: u32) -> String {
    format!("{}.{:01} MB", kib / 1024, kib % 1024 * 10 / 1024)
}

fn loading_spinner(key: &'static str) -> View<AppMsg> {
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

fn checkmark(key: &'static str, color: rs_lvgl_ui::Color) -> View<AppMsg> {
    // Reuse the icon font already embedded for the launcher. The outer box
    // keeps its layout at 24 px while LVGL scales the glyph around its centre.
    let font = unsafe { rs_lvgl_ui::Font::from_raw(core::ptr::addr_of!(rs_lvgl::sys::lv_font_montserrat_48)) };
    rs_lvgl_ui::obj()
        .key(Key::Named(key))
        .scrollable(false)
        .clickable(false)
        .style(Style::neutral().width(Size::Px(24)).height(Size::Px(24)))
        .child(text(rs_lvgl_ui::symbol::OK, font, color).style(
            Style::default().align(Align::Center)
                .transform_scale_x(128).transform_scale_y(128)
                .transform_pivot_x(24).transform_pivot_y(24),
        ))
}

pub fn view(hub: &Hub, session: &Session) -> View<AppMsg> {
    let s = &hub.settings;
    let info = session.info;
    let body = match hub.settings_page {
        Page::Root => list("categories")
            .child(entry(
                Key::Named("character"), "更换团子",
                format!("{}  ›", session.skins.iter().find(|choice| choice.id == session.selected_skin)
                    .map(|choice| choice.name.as_str()).unwrap_or("读取中")),
                AppMsg::SettingsPage(Page::Character),
            ))
            .child(entry(
                Key::Named("display"),
                "显示与电源",
                format!("{}%  ›", s.brightness),
                AppMsg::SettingsPage(Page::Display),
            ))
            .child(entry(Key::Named("audio"),"声音",format!("{}%  ›",s.audio_volume),AppMsg::SettingsPage(Page::Audio)))
            .child(entry(
                Key::Named("feedback"),
                "灯光与震动",
                "›",
                AppMsg::SettingsPage(Page::Feedback),
            ))
            .child(entry(
                Key::Named("wireless"),
                "无线连接",
                info.wifi.label(),
                AppMsg::SettingsPage(Page::Wireless),
            ))
            .child(entry(
                Key::Named("camera"),
                "相机",
                "›",
                AppMsg::SettingsPage(Page::Camera),
            ))
            .child(entry(
                Key::Named("storage"),
                "存储",
                if info.storage_ready {
                    size(info.data_free_kib)
                } else {
                    "读取中".into()
                },
                AppMsg::SettingsPage(Page::Storage),
            ))
            .child(entry(
                Key::Named("update"),
                "系统更新",
                if session.update.phase==crate::update::Phase::Available {"有新版本"} else {"›"},
                AppMsg::SettingsPage(Page::Update),
            ))
            .child(entry(
                Key::Named("system"),
                "关于设备",
                "›",
                AppMsg::SettingsPage(Page::System),
            )),
        Page::Character => {
            let mut body = list("character-choices");
            for (index, choice) in session.skins.iter().enumerate() {
                let selected = choice.id == session.selected_skin;
                let loading = session.loading_skin.as_deref() == Some(choice.id.as_str());
                let mut choice_view = entry(
                    Key::Id(index as u64), choice.name.clone(),
                    "",
                    AppMsg::SelectSkin(choice.id.clone()),
                ).style(Style::default().height(Size::Px(56))
                    .border(if selected { ACCENT } else { CARD }, 2)
                    .bg(if selected { CARD_HI } else { CARD }))
                 .clickable(!info.busy && session.loading_skin.is_none() && info.storage_ready && !selected);
                if loading {
                    choice_view = choice_view.child(
                        rs_lvgl_ui::row([]).key(Key::Named("skin-loading"))
                            .scrollable(false).clickable(false)
                            .style(Style::neutral().width(Size::Content).height(Size::Px(24)).gap(8)
                                .flex_align(FlexAlign::Center, FlexAlign::Center).align(Align::RightMid))
                            .child(loading_spinner("skin-spinner"))
                            .child(text("切换中", font_20(), TEXT)),
                    );
                } else if selected {
                    choice_view = choice_view.child(checkmark("skin-selected", ACCENT)
                        .style(Style::default().align(Align::RightMid)));
                }
                body = body.child(choice_view);
            }
            if !info.storage_ready {
                body = body.child(note("character-loading", "正在读取团子形象"));
            } else if session.skins.len() <= 1 {
                body = body.child(note("character-empty", "安装团子形象后可在这里切换"));
            }
            if let Some(message) = session.message {
                body = body.child(note("character-message", message));
            }
            body
        }
        Page::Display => list("display-controls")
            .child(level_slider("brightness","亮度",10,s.brightness,AppMsg::Brightness))
            .child(entry(
                Key::Named("screen-off"),
                "熄屏 30 秒",
                "轻触可唤醒",
                AppMsg::ScreenOff,
            ))
            .child(row(
                "deep-sleep",
                "深度睡眠",
                text("暂未启用", font_20(), MUTED),
            )),
        Page::Audio => list("audio-controls")
            .child(level_slider("volume","音量",0,s.audio_volume,AppMsg::AudioVolume))
            .child(row("audio-mute","静音",switch("switch",s.audio_muted,AppMsg::AudioMute)))
            .child(row("audio-state","音频状态",text(match info.audio {
                crate::audio::State::Off=>"已停止",crate::audio::State::Starting=>"正在启动",crate::audio::State::Ready=>"已就绪",
                crate::audio::State::Playing=>"正在播放",crate::audio::State::Recording=>"正在录音",crate::audio::State::Fault=>"音频故障",
            },font_20(),MUTED))),
        Page::Feedback => list("feedback-controls")
            .child(row(
                "lights",
                "灯光",
                switch("switch", s.lights, AppMsg::Lights),
            ))
            .child(row(
                "haptics",
                "震动",
                switch("switch", s.haptics, AppMsg::Haptics),
            )),
        Page::Wireless => list("wireless-controls")
            .child(entry(
                Key::Named("wifi"),
                "Wi-Fi",
                format!("{}  ›", info.wifi.label()),
                AppMsg::SettingsPage(Page::Wifi),
            ))
            .child(row(
                "bluetooth",
                "蓝牙",
                switch(
                    "switch",
                    matches!(
                        info.bluetooth,
                        crate::radio::BluetoothState::Discoverable
                            | crate::radio::BluetoothState::Connected
                            | crate::radio::BluetoothState::Streaming
                            | crate::radio::BluetoothState::Starting
                    ),
                    AppMsg::Bluetooth,
                )
                .clickable(!info.busy),
            ))
            .child(row(
                "bluetooth-state",
                "蓝牙状态",
                text(info.bluetooth.label(), font_20(), MUTED),
            )),
        Page::Wifi => {
            let state = match info.ip {
                Some(b) => format!(
                    "{} · {}.{}.{}.{}",
                    info.wifi.label(),
                    b[0],
                    b[1],
                    b[2],
                    b[3]
                ),
                None => info.wifi.label().into(),
            };
            let mut body = list("wifi-networks")
                .child(row(
                    "wifi-enabled",
                    "Wi-Fi",
                    switch("switch", info.wifi.enabled(), AppMsg::Wifi).clickable(!info.busy),
                ))
                .child(note("wifi-state", state));
            if let Some(ssid) = &session.wifi_saved.current {
                body = body.child(note("wifi-current", format!("当前：{}", ssid_label(ssid))));
            }
            if session.wifi_saved.loaded {
                body = body.child(row("wifi-automatic", "自动连接", switch(
                    "switch", session.wifi_saved.automatic, AppMsg::WifiAutomatic,
                ).clickable(!info.busy)));
            }
            if session.wifi_saved.error {
                body = body.child(note("wifi-save-error", "网络未保存，请重试"));
            }
            if !session.wifi_saved.networks.is_empty() {
                body = body.child(note("wifi-known-title", "已保存网络"));
                for (index, ssid) in session.wifi_saved.networks.iter().enumerate() {
                    body = body.child(entry(Key::Id(1000 + index as u64), ssid_label(ssid),
                        if session.wifi_saved.current.as_ref() == Some(ssid) { "已连接" } else { "›" },
                        AppMsg::WifiChoose(crate::radio::Network { ssid: ssid.clone(), channel: 0, rssi: 0, auth: "WPA2".into() }),
                    ).clickable(!info.busy));
                }
            }
            if info.wifi.enabled() {
                body = body.child(
                    entry(
                        Key::Named("scan"),
                        if info.busy {
                            "操作进行中"
                        } else {
                            "搜索附近网络"
                        },
                        "›",
                        AppMsg::WifiScan,
                    )
                    .clickable(!info.busy),
                );
            }
            if let Some(message) = session.message {
                body = body.child(note("wifi-message", message));
            }
            for (index, network) in session
                .networks
                .iter()
                .enumerate()
                .filter(|(_, network)| info.wifi.enabled() && !session.wifi_saved.networks.contains(&network.ssid))
            {
                let name = ssid_label(&network.ssid);
                body = body.child(
                    entry(
                        Key::Id(index as u64),
                        name,
                        format!("{} dBm", network.rssi),
                        AppMsg::WifiChoose(network.clone()),
                    )
                    .clickable(!info.busy && !network.ssid.is_empty()),
                );
            }
            body
        }
        Page::KnownNetwork => {
            let name = session.selected.as_ref().map(|n| ssid_label(&n.ssid)).unwrap_or_default();
            list("wifi-known-network")
                .child(note("wifi-known-name", name))
                .child(entry(Key::Named("wifi-known-connect"), "连接此网络", "›", AppMsg::WifiSavedConnect).clickable(!info.busy))
                .child(entry(Key::Named("wifi-known-key"), "重新输入密码", "›", AppMsg::WifiReenterPassword).clickable(!info.busy))
                .child(entry(Key::Named("wifi-known-forget"), "忘记此网络", "›", AppMsg::WifiForget).clickable(!info.busy))
        }
        Page::Password => return password(session),
        Page::Camera => {
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
        Page::Storage => list("storage-info")
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
            )),
        Page::Update => update_page(hub, session),
        Page::Proxy => return proxy_page(session),
        Page::System => list("system-info")
            .child(row("model", "设备", text("ESP-Mosaico", font_20(), MUTED)))
            .child(row(
                "version",
                "固件版本",
                text(env!("CARGO_PKG_VERSION"), font_20(), MUTED),
            ))
            .child(row(
                "uptime",
                "运行时间",
                text(format!("{} 分", hub.about.uptime_s / 60), font_20(), MUTED),
            ))
            .child(row(
                "heap",
                "可用内存",
                text(size(hub.about.free_kib), font_20(), MUTED),
            ))
            .child(row(
                "ota",
                "固件状态",
                text(
                    if info.candidate {
                        "启动检查中"
                    } else {
                        "已确认"
                    },
                    font_20(),
                    MUTED,
                ),
            ))
            .child(row(
                "slot",
                "启动槽",
                text(
                    match info.boot_slot {
                        Some(0) => "A",
                        Some(1) => "B",
                        _ => "未知",
                    },
                    font_20(),
                    MUTED,
                ),
            )),
    };
    let body = if hub.settings_page == Page::Wireless {
        if let Some(message) = session.message { body.child(note("wireless-message", message)) } else { body }
    } else { body };
    let mut view = page(AppId::Settings.key(), hub.settings_page.title(), body);
    if hub.settings_page != Page::Root {
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
    if hub.settings_page == Page::Character && !info.saving && !info.save_error {
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
fn password(session: &Session) -> View<AppMsg> {
    let name = session
        .selected
        .as_ref()
        .map(|n| ssid_label(&n.ssid))
        .unwrap_or_default();
    page(
        AppId::Settings.key(),
        "连接网络",
        text(fit_text(&name, 400), font_20(), MUTED)
            .key(Key::Named("ssid"))
            .style(
                Style::default()
                    .width(Size::Px(400))
                    .align(Align::TopMid)
                    .offset(0, 78),
            ),
    )
    .child(
        rs_lvgl_ui::textarea(&session.password.0)
            .key(Key::Named("wifi-password"))
            .keyboard_id(701)
            .password(session.password_hidden)
            .placeholder("输入 Wi-Fi 密码")
            .style(
                card()
                    .width(Size::Px(304))
                    .height(Size::Px(52))
                    .font(font_24())
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(40, 112),
            )
            .on_text_changed(|s| AppMsg::WifiPassword(Password(s.into()))),
    )
    .child(
        rs_lvgl_ui::button(if session.password_hidden {
            "显示"
        } else {
            "隐藏"
        })
        .key(Key::Named("password-visibility"))
        .style(
            card()
                .width(Size::Px(88))
                .height(Size::Px(52))
                .font(font_20())
                .fg(TEXT)
                .shadow_width(0)
                .align(Align::TopLeft)
                .offset(352, 112),
        )
        .on_click(AppMsg::WifiPasswordHidden(!session.password_hidden)),
    )
    .child(
        rs_lvgl_ui::button("取消")
            .key(Key::Named("cancel"))
            .style(
                card()
                    .width(Size::Px(188))
                    .height(Size::Px(44))
                    .font(font_24())
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(40, 177),
            )
            .on_click(AppMsg::SettingsBack),
    )
    .child(
        rs_lvgl_ui::button("连接")
            .key(Key::Named("connect"))
            .style(
                card()
                    .width(Size::Px(188))
                    .height(Size::Px(44))
                    .font(font_24())
                    .bg(ACCENT)
                    .fg(TEXT)
                    .align(Align::TopLeft)
                    .offset(252, 177),
            )
            .on_click(AppMsg::WifiConnect),
    )
    .child(
        rs_lvgl_ui::keyboard(701)
            .key(Key::Named("wifi-keyboard"))
            .style(
                Style::default()
                    .width(Size::Px(400))
                    .height(Size::Px(184))
                    .bg(BG)
                    .align(Align::TopMid)
                    .offset(0, 238),
            )
            .part_style(
                Part::Items,
                Style::neutral()
                    .bg(CARD_HI)
                    .fg(TEXT)
                    .font(font_20())
                    .radius(8),
            )
            .part_state_style(Part::Items, State::PRESSED, Style::default().bg(ACCENT)),
    )
    .child(
        text(session.message.unwrap_or(""), font_20(), MUTED)
            .key(Key::Named("password-error"))
            .style(Style::default().align(Align::TopMid).offset(0, 428)),
    )
}

fn update_page(hub: &Hub, session: &Session) -> View<AppMsg> {
    use crate::update::{Command, Phase};
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
    body = body.child(row(
        "proxy-enabled", "手动代理",
        switch("switch", hub.settings.update_proxy_enabled, AppMsg::UpdateProxyToggle)
            .clickable(!busy && !session.info.busy),
    )).child(entry(
        Key::Named("proxy-address"), "代理地址",
        hub.settings.update_proxy.map(crate::update::HttpProxy::address).unwrap_or_else(|| "未配置  ›".into()),
        AppMsg::SettingsPage(Page::Proxy),
    ).clickable(!busy && !session.info.busy));
    body
}

fn proxy_page(session: &Session) -> View<AppMsg> {
    page(AppId::Settings.key(), "更新代理",
        text("HTTP 无认证 · IPv4:端口", font_20(), MUTED)
            .style(Style::default().width(Size::Px(400)).align(Align::TopMid).offset(0,78)))
    .child(rs_lvgl_ui::textarea(&session.proxy_address)
        .key(Key::Named("proxy-endpoint")).keyboard_id(702)
        .placeholder("例如 10.0.0.1:7890")
        .style(card().width(Size::Px(400)).height(Size::Px(52)).font(font_24())
            .fg(TEXT).align(Align::TopLeft).offset(40,112))
        .on_text_changed(|s|AppMsg::UpdateProxyText(s.into())))
    .child(rs_lvgl_ui::button("取消").key(Key::Named("proxy-cancel"))
        .style(card().width(Size::Px(188)).height(Size::Px(44)).font(font_24())
            .fg(TEXT).align(Align::TopLeft).offset(40,177))
        .on_click(AppMsg::SettingsBack))
    .child(rs_lvgl_ui::button("保存并启用").key(Key::Named("proxy-apply"))
        .style(card().width(Size::Px(188)).height(Size::Px(44)).font(font_24())
            .bg(ACCENT).fg(TEXT).align(Align::TopLeft).offset(252,177))
        .on_click(AppMsg::UpdateProxyApply))
    .child(rs_lvgl_ui::keyboard(702).key(Key::Named("proxy-keyboard"))
        .style(Style::default().width(Size::Px(400)).height(Size::Px(184))
            .bg(BG).align(Align::TopMid).offset(0,238))
        .part_style(Part::Items, Style::neutral().bg(CARD_HI).fg(TEXT).font(font_20()).radius(8))
        .part_state_style(Part::Items, State::PRESSED, Style::default().bg(ACCENT)))
    .child(text(session.message.unwrap_or("HTTP / mixed 代理，仅用于系统更新"), font_20(), MUTED)
        .key(Key::Named("proxy-error"))
        .style(Style::default().width(Size::Px(400)).align(Align::TopMid).offset(0,428)))
}
