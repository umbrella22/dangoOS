//! Live sensor page.
use alloc::format;
use alloc::string::String;

use rs_lvgl_ui::{Align, Color, GridTrack, Key, Part, ScrollDir, ScrollbarMode, Size, Style, View};

use crate::companion::behavior::affect::ModuleKind;
use crate::shell::messages::AppMsg;
use {crate::services::input::snapshot::MagnetState, crate::services::input::snapshot::SlotState};
use {crate::shell::model::Hub, crate::shell::navigation::route::AppId};

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
// ----------------------------------------------------------------- sensors

/// The scroll viewport ends above the system gesture band at y=420.
const SENSOR_CARD_H: i16 = 108;
const SENSOR_Y: i16 = 80;

fn sensor_card(
    key: &'static str,
    title: &'static str,
    lines: [String; 2],
    bar: Option<(u8, Color)>,
) -> View<AppMsg> {
    let mut card_node = rs_lvgl_ui::column([])
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(CARD)
                .radius(RADIUS)
                .pad(10)
                .pad_left(18)
                .pad_right(18)
                .width(Size::Px(196))
                .height(Size::Px(SENSOR_CARD_H))
                .gap(2),
        )
        .child(text(title, font_20(), MUTED).key(Key::Named("title")));
    let [first, second] = lines;
    card_node = card_node.child(text(first, font_24(), TEXT).key(Key::Named("value")));
    match bar {
        Some((value, color)) => {
            card_node = card_node.child(
                rs_lvgl_ui::bar(0, 100, value as i32)
                    .key(Key::Named("bar"))
                    .style(
                        Style::default()
                            .width(Size::Percent(100))
                            .height(Size::Px(8))
                            .offset(0, 8)
                            .bg(CARD_HI)
                            .radius(4),
                    )
                    .part_style(Part::Indicator, Style::default().bg(color).radius(4)),
            );
        }
        None => {
            card_node = card_node.child(text(second, font_20(), MUTED).key(Key::Named("detail")));
        }
    }
    card_node
}

fn dash() -> String {
    String::from("--")
}

fn slot_name(slot: SlotState) -> &'static str {
    match slot {
        SlotState::Empty => "空",
        SlotState::Module(ModuleKind::Interaction) => "交互板",
        SlotState::Module(ModuleKind::Camera) => "摄像头",
    }
}

/// Sensor readings and camera/radio service snapshots in one scrollable grid.
pub fn sensors(hub: &Hub) -> View<AppMsg> {
    let snapshot = &hub.snapshot;
    let tilt = match (snapshot.tilt_deg(), snapshot.accel_mg) {
        (Some(deg), Some([x, y, z])) => [
            format!("倾角 {}°", deg),
            format!("{} / {} / {} mg", x, y, z),
        ],
        _ => [dash(), String::from("等待数据")],
    };
    let gyro = match snapshot.gyro_dps {
        Some([x, y, z]) => [format!("{} / {} / {}", x, y, z), String::from("°/s")],
        None => [dash(), String::from("°/s")],
    };
    let light = match snapshot.light {
        Some(value) => ([format!("{}", value), String::new()], Some((value, YELLOW))),
        None => ([dash(), String::from("无传感器")], None),
    };
    let battery = match snapshot.battery {
        Some(value) => ([format!("{}%", value), String::new()], Some((value, GREEN))),
        None => ([dash(), String::from("读取中")], None),
    };
    let slots = [
        format!("左 {}", slot_name(snapshot.left)),
        format!("右 {}", slot_name(snapshot.right)),
    ];
    let presence = match snapshot.presence {
        Some(true) => "有人",
        Some(false) => "无人",
        None => "--",
    };
    let magnet = match snapshot.magnet {
        MagnetState::Away => "磁铁 远离",
        MagnetState::Near => "磁铁 靠近",
        MagnetState::Contact => "磁铁 贴合",
    };

    let camera_slot = if snapshot.left == SlotState::Module(ModuleKind::Camera) {
        Some("左侧已接入")
    } else if snapshot.right == SlotState::Module(ModuleKind::Camera) {
        Some("右侧已接入")
    } else {
        None
    };
    let camera_state = match hub.camera.state {
        crate::services::camera::State::Off => {
            if camera_slot.is_some() {
                "待机"
            } else {
                "未接入"
            }
        }
        crate::services::camera::State::Starting => "正在启动",
        crate::services::camera::State::Preview => "正在采集",
        crate::services::camera::State::Fault => "相机故障",
    };
    let camera_detail = if hub.camera.pid != 0 && camera_slot.is_some() {
        hub.camera.sensor()
    } else {
        camera_slot.unwrap_or("未检测到模块")
    };
    let wifi_detail = match hub
        .radio
        .ip
        .filter(|_| hub.radio.wifi == crate::services::radio::WifiState::Connected)
    {
        Some(b) => format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]),
        None => "2.4 GHz".into(),
    };

    let cards = [
        sensor_card("tilt", "姿态", tilt, None),
        sensor_card("gyro", "角速度", gyro, None),
        sensor_card("light", "光线", light.0, light.1),
        sensor_card("battery", "电量", battery.0, battery.1),
        sensor_card("slots", "模块", slots, None),
        sensor_card(
            "camera",
            "相机",
            [camera_state.into(), camera_detail.into()],
            None,
        ),
        sensor_card(
            "wifi",
            "Wi-Fi",
            [hub.radio.wifi.label().into(), wifi_detail],
            None,
        ),
        sensor_card(
            "bluetooth",
            "蓝牙",
            [
                hub.radio.bluetooth.label().into(),
                "ESP-Mosaico Audio".into(),
            ],
            None,
        ),
        sensor_card(
            "presence",
            "状态",
            [String::from(presence), String::from(magnet)],
            None,
        )
        .style(Style::default().width(Size::Px(COLUMN_W))),
    ];
    let grid = rs_lvgl_ui::grid(
        [GridTrack::Px(196), GridTrack::Px(196)],
        [
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
        ],
        cards.into_iter().enumerate().map(|(i, card)| {
            card.grid_cell(
                (i % 2) as u16,
                (i / 2) as u16,
                if i == 8 { 2 } else { 1 },
                1,
            )
        }),
    )
    .key(Key::Named("grid"))
    .scrollable(true)
    .scroll_dir(ScrollDir::VER)
    .scrollbar_mode(ScrollbarMode::Auto)
    .style(
        Style::neutral()
            .width(Size::Px(COLUMN_W))
            .height(Size::Px(340))
            .gap(8)
            .align(Align::TopMid)
            .offset(0, SENSOR_Y),
    );

    page(AppId::Sensors.key(), AppId::Sensors.title(), grid)
}
