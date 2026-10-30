//! LVGL views for the hub pages (RFC 0006): the launcher and the three native
//! apps. The fox home page stays in [`super::fox_widget`].
//!
//! Layout rules for the 480x480 round-cornered panel:
//!
//! - nothing interactive below [`gesture::BOTTOM_BAND_Y`] or left of
//!   [`gesture::LEFT_BAND_X`]: those bands belong to the system gestures;
//! - content stays inside a 60 px corner safe area, so pages are centred
//!   columns at most 400 px wide;
//! - colours and sizes: black field, 30 px titles, 24 px labels, 22 px
//!   values, orange accent (the earlier Mosaico UI design tokens).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use rs_lvgl_ui::{
    symbol, Align, Color, FlexAlign, Font, GridTrack, ImageSource, Key, Part, Size, State, Style,
    ScrollDir, ScrollbarMode, TextAlign, View,
};

use super::hub_font::{HUB_20, HUB_24, HUB_30, HUB_DIGITS_64};
use super::AppMsg;
use crate::behavior::affect::ModuleKind;
use crate::hub::countdown::{format_mm_ss, Phase, PRESETS_MIN};
use crate::hub::registry::{AppEntry, Registry};
use crate::hub::snapshot::{MagnetState, SlotState};
use crate::hub::{AppId, Hub};

/// Design tokens.
pub mod tokens {
    use rs_lvgl_ui::Color;

    pub const BG: Color = Color::rgb(0x00, 0x00, 0x00);
    pub const CARD: Color = Color::rgb(0x18, 0x18, 0x19);
    pub const CARD_HI: Color = Color::rgb(0x28, 0x28, 0x29);
    pub const TEXT: Color = Color::rgb(0xFC, 0xFC, 0xFF);
    pub const TITLE: Color = Color::rgb(0xD6, 0xD6, 0xDE);
    pub const MUTED: Color = Color::rgb(0x91, 0x91, 0x9B);
    pub const WEAK: Color = Color::rgb(0x59, 0x59, 0x59);
    pub const ACCENT: Color = Color::rgb(0xFF, 0x4C, 0x01);
    pub const GREEN: Color = Color::rgb(0x63, 0xC2, 0x70);
    pub const YELLOW: Color = Color::rgb(0xFF, 0xB0, 0x20);
    pub const BLUE: Color = Color::rgb(0x3D, 0x8B, 0xFF);

    /// Width of the content column.
    pub const COLUMN_W: i16 = 400;
    /// Top of the content, below the title.
    pub const CONTENT_Y: i16 = 92;
    pub const ROW_H: i16 = 68;
    pub const RADIUS: i16 = 20;
}

use tokens::*;

/// Terminal battery warning, independent of mounted apps, skins and Lua.
pub(super) fn low_battery() -> View<AppMsg> {
    rs_lvgl_ui::obj().key(Key::Named("low-battery-root")).scrollable(false)
        .style(Style::neutral().bg(BG).width(Size::Percent(100)).height(Size::Percent(100)))
        .child(rs_lvgl_ui::obj().style(Style::neutral().width(Size::Px(100)).height(Size::Px(56))
            .align(Align::Center).offset(-3, -76).radius(9).border(MUTED, 3))
            .child(rs_lvgl_ui::obj().style(Style::neutral().width(Size::Px(8)).height(Size::Px(22))
                .bg(ACCENT).radius(2).align(Align::LeftMid).offset(9, 0))))
        .child(rs_lvgl_ui::obj().style(Style::neutral().width(Size::Px(6)).height(Size::Px(20))
            .bg(MUTED).radius(2).align(Align::Center).offset(52, -76)))
        .child(text("低电量", font_30(), TEXT).style(Style::default().align(Align::Center).offset(0, 2)))
        .child(text("即将关机，请连接电源", font_20(), MUTED).style(Style::default().align(Align::Center).offset(0, 48)))
}

pub(super) fn font_20() -> Font {
    HUB_20.font()
}
pub(super) fn font_24() -> Font {
    HUB_24.font()
}
fn font_30() -> Font {
    HUB_30.font()
}
fn font_digits() -> Font {
    HUB_DIGITS_64.font()
}
fn font_icon() -> Font {
    // SAFETY: a static LVGL font enabled by the vendored configuration.
    unsafe { Font::from_raw(core::ptr::addr_of!(rs_lvgl::sys::lv_font_montserrat_48)) }
}

pub(super) fn text(content: impl Into<String>, font: Font, color: Color) -> View<AppMsg> {
    rs_lvgl_ui::label(content).style(Style::neutral().font(font).fg(color))
}

/// A full-screen page: black field, centred title, home indicator.
pub(super) fn page(key: &'static str, title: &'static str, body: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::obj()
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(BG)
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            text(title, font_30(), TITLE)
                .key(Key::Named("title"))
                .style(Style::default().align(Align::TopMid).offset(0, 30)),
        )
        .child(body)
        .child(home_indicator())
}

/// The pill at the bottom edge: "swipe up from here".
fn home_indicator() -> View<AppMsg> {
    rs_lvgl_ui::obj().key(Key::Named("home-pill")).style(
        Style::neutral()
            .bg(WEAK)
            .radius(3)
            .width(Size::Px(96))
            .height(Size::Px(6))
            .align(Align::BottomMid)
            .offset(0, -16),
    )
}

pub(super) fn card() -> Style {
    Style::neutral().bg(CARD).radius(RADIUS).pad(20)
}

/// A 68 px row: label on the left, `trailing` on the right.
pub(super) fn row(key: &'static str, label: &'static str, trailing: View<AppMsg>) -> View<AppMsg> {
    rs_lvgl_ui::row([])
        .key(Key::Named(key))
        .scrollable(false)
        .style(
            card()
                .pad_top(0)
                .pad_bottom(0)
                .width(Size::Percent(100))
                .height(Size::Px(ROW_H))
                .flex_align(FlexAlign::SpaceBetween, FlexAlign::Center),
        )
        .child(text(label, font_24(), TEXT).key(Key::Named("label")))
        .child(trailing)
}

pub(super) fn switch(key: &'static str, on: bool, msg: fn(bool) -> AppMsg) -> View<AppMsg> {
    rs_lvgl_ui::switch(on)
        .key(Key::Named(key))
        .style(
            Style::default()
                .width(Size::Px(64))
                .height(Size::Px(34))
                .bg(CARD_HI),
        )
        .part_state_style(Part::Indicator, State::CHECKED, Style::default().bg(ACCENT))
        .part_style(Part::Knob, Style::default().bg(TEXT))
        .on_value_changed(move |v| msg(v != 0))
}

// ---------------------------------------------------------------- launcher

struct Tile {
    app: AppId,
    icon: &'static str,
    color: Color,
}

const NATIVE_TILES: [Tile; 4] = [
    Tile {
        app: AppId::Settings,
        icon: symbol::SETTINGS,
        color: Color::rgb(0x48, 0x48, 0x4E),
    },
    Tile {
        app: AppId::Sensors,
        icon: symbol::GPS,
        color: BLUE,
    },
    Tile {
        app: AppId::Timer,
        icon: symbol::BELL,
        color: ACCENT,
    },
    Tile {app:AppId::Camera,icon:symbol::IMAGE,color:GREEN},
];

/// Launcher tiles per row.
const TILES_PER_ROW: usize = 3;
/// Rows that fit without scrolling.
const FIXED_ROWS: usize = 2;
/// Height of the scrolling grid when there are more rows.
const GRID_H: i16 = 330;

/// Reconciler key of a Lua app's tile or page: a hash of its id, so a
/// rescan that moves the app keeps its widgets.
pub fn lua_key(id: &str) -> Key {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in id.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    Key::Id(h)
}

/// What a tile shows.
enum TileFace<'a> {
    Symbol(&'static str, Color),
    Icon(&'a crate::hub::registry::Icon),
    /// A Lua app without an icon: the first letter of its name.
    Letter(String, Color),
}

fn tile(
    key: Key,
    app: AppId,
    face: TileFace,
    name: &str,
    dim: bool,
    identity: Option<crate::hub::uninstall::Identity>,
) -> View<AppMsg> {
    let square = rs_lvgl_ui::obj()
        .key(Key::Named("icon"))
        .scrollable(false)
        // Plain objects are clickable by default; the tap must land on the
        // tile, which owns the click handler.
        .clickable(false);
    let (bg, child) = match face {
        TileFace::Symbol(glyph, color) => (color, text(glyph, font_icon(), TEXT)),
        TileFace::Letter(letter, color) => (color, text(letter, font_30(), TEXT)),
        TileFace::Icon(icon) => (
            BG,
            rs_lvgl_ui::image(ImageSource::Rgb565 {
                width: icon.width,
                height: icon.height,
                data: icon.rgb565,
            })
            .clickable(false),
        ),
    };
    let square = square
        .style(
            Style::neutral()
                .bg(bg)
                .radius(28)
                .width(Size::Px(96))
                .height(Size::Px(96))
                .opacity(if dim { 110 } else { 255 }),
        )
        .child(
            child
                .key(Key::Named("glyph"))
                .style(Style::default().align(Align::Center)),
        );
    rs_lvgl_ui::column([])
        .key(key)
        .scrollable(false)
        .clickable(true)
        .on_event(move |event| match event {
            rs_lvgl_ui::Event::ShortClicked => {
                Some(identity.map(AppMsg::OpenLua).unwrap_or(AppMsg::Open(app)))
            }
            rs_lvgl_ui::Event::LongPressed => identity.map(AppMsg::ManageAsk),
            rs_lvgl_ui::Event::PressLost | rs_lvgl_ui::Event::ScrollBegin => {
                Some(AppMsg::ManageCancel)
            }
            _ => None,
        })
        .style(
            Style::neutral()
                .width(Size::Px(120))
                .height(Size::Content)
                .gap(8)
                .flex_align(FlexAlign::Start, FlexAlign::Center),
        )
        .child(square)
        .child(
            text(fit_name(name), font_24(), if dim { WEAK } else { TEXT }).key(Key::Named("name")),
        )
}

/// `name` shortened with "…" to fit under a 120 px tile at 24 px.
fn fit_name(name: &str) -> String {
    fit_text(name, 120)
}
pub(super) fn fit_text(name: &str, max: u32) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let name = clean.as_str();
    let w = |c: char| super::hub_font::advance(&HUB_24, c).unwrap_or(24) as u32;
    if name.chars().map(w).sum::<u32>() <= max {
        return name.into();
    }
    let mut out = String::new();
    let mut used = w('…');
    for c in name.chars() {
        if used + w(c) > max {
            break;
        }
        used += w(c);
        out.push(c);
    }
    out.push('…');
    out
}

/// Tile colours for Lua apps without an icon.
const LETTER_COLORS: [Color; 4] = [
    Color::rgb(0x2E, 0x9E, 0x5B),
    Color::rgb(0xC8, 0x8A, 0x2E),
    Color::rgb(0x8A, 0x4F, 0xD8),
    Color::rgb(0x2E, 0x8E, 0xA8),
];

/// The app grid: the native apps, then the registry's Lua apps, three per
/// row. Beyond two rows the grid scrolls.
pub fn launcher(registry: &Registry) -> View<AppMsg> {
    let mut tiles: Vec<View<AppMsg>> = NATIVE_TILES
        .iter()
        .map(|t| {
            tile(
                Key::Named(t.app.key()),
                t.app,
                TileFace::Symbol(t.icon, t.color),
                t.app.title(),
                false,
                None,
            )
        })
        .collect();
    for (i, entry) in registry.apps.iter().enumerate().take(u8::MAX as usize) {
        let face = match &entry.icon {
            Some(icon) => TileFace::Icon(icon),
            None => TileFace::Letter(
                entry
                    .title
                    .chars()
                    .next()
                    .map(String::from)
                    .unwrap_or_default(),
                LETTER_COLORS[i % LETTER_COLORS.len()],
            ),
        };
        tiles.push(tile(
            lua_key(&entry.app.id),
            AppId::Lua(i as u8),
            face,
            &entry.title,
            !entry.runnable(),
            crate::hub::uninstall::Identity::new(&entry.app.id, entry.origin),
        ));
    }
    let row_count = tiles.len().div_ceil(TILES_PER_ROW);
    let mut rows = Vec::with_capacity(row_count);
    let mut tiles = tiles.into_iter();
    for r in 0..row_count {
        rows.push(
            rs_lvgl_ui::row([])
                .key(Key::Id(r as u64))
                .scrollable(false)
                .style(
                    Style::neutral()
                        .width(Size::Content)
                        .height(Size::Content)
                        .gap(16),
                )
                .children(tiles.by_ref().take(TILES_PER_ROW)),
        );
    }
    let grid = rs_lvgl_ui::column([]).key(Key::Named("tiles"));
    let grid = if row_count <= FIXED_ROWS {
        grid.scrollable(false).style(
            Style::neutral()
                .width(Size::Content)
                .height(Size::Content)
                .gap(20)
                .flex_align(FlexAlign::Start, FlexAlign::Center)
                .align(Align::Center)
                .offset(0, 0),
        )
    } else {
        grid.scrollable(true).style(
            Style::neutral()
                .width(Size::Percent(100))
                .height(Size::Px(GRID_H))
                .gap(20)
                .flex_align(FlexAlign::Start, FlexAlign::Center)
                .align(Align::TopMid)
                .offset(0, 84),
        )
    };

    rs_lvgl_ui::obj()
        .key(Key::Named("launcher"))
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(BG)
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            text("应用", font_30(), TITLE)
                .key(Key::Named("title"))
                .style(Style::default().align(Align::TopMid).offset(0, 30)),
        )
        .child(grid.children(rows))
        .child(
            text("上滑回到狐狸 · 左缘右滑返回", font_20(), WEAK)
                .key(Key::Named("hint"))
                .style(Style::default().align(Align::BottomMid).offset(0, -44)),
        )
        .child(home_indicator())
}

/// A Lua app's page. The script draws the whole screen once it opens the
/// display (the UI task then stops rendering LVGL here); until then this
/// shows the name and a hint, or why the app cannot run.
pub fn lua_app(entry: Option<&AppEntry>) -> View<AppMsg> {
    let (key, title, status) = match entry {
        Some(e) if e.runnable() => (
            lua_key(&e.app.id),
            e.title.clone(),
            String::from("正在启动…"),
        ),
        Some(e) => (
            lua_key(&e.app.id),
            e.title.clone(),
            alloc::format!("不支持：缺少 {}", e.missing.join(", ")),
        ),
        None => (
            Key::Named("app-lua"),
            String::from("Lua"),
            String::from("应用已移除"),
        ),
    };
    rs_lvgl_ui::obj()
        .key(key)
        .scrollable(false)
        .style(
            Style::neutral()
                .bg(Color::rgb(0, 0, 0))
                .width(Size::Percent(100))
                .height(Size::Percent(100)),
        )
        .child(
            text(title, font_30(), TITLE)
                .key(Key::Named("title"))
                .style(Style::default().align(Align::Center).offset(0, -20)),
        )
        .child(
            text(status, font_20(), WEAK)
                .key(Key::Named("status"))
                .style(Style::default().align(Align::Center).offset(0, 24)),
        )
        .child(home_indicator())
}

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
        crate::camera::State::Off => {
            if camera_slot.is_some() { "待机" } else { "未接入" }
        }
        crate::camera::State::Starting => "正在启动",
        crate::camera::State::Preview => "正在采集",
        crate::camera::State::Fault => "相机故障",
    };
    let camera_detail = if hub.camera.pid != 0 && camera_slot.is_some() {
        hub.camera.sensor()
    } else {
        camera_slot.unwrap_or("未检测到模块")
    };
    let wifi_detail = match hub.radio.ip.filter(|_| {
        hub.radio.wifi == crate::radio::WifiState::Connected
    }) {
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
            "camera", "相机", [camera_state.into(), camera_detail.into()], None,
        ),
        sensor_card(
            "wifi", "Wi-Fi", [hub.radio.wifi.label().into(), wifi_detail], None,
        ),
        sensor_card(
            "bluetooth", "蓝牙", [hub.radio.bluetooth.label().into(), "ESP-Mosaico Audio".into()], None,
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
        cards
            .into_iter()
            .enumerate()
            .map(|(i, card)| {
                card.grid_cell((i % 2) as u16, (i / 2) as u16, if i == 8 { 2 } else { 1 }, 1)
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

/// The view for one app.
pub fn app(hub: &Hub, registry: &Registry, app: AppId) -> View<AppMsg> {
    match app {
        AppId::Settings => settings_pages(hub,&crate::hub::settings::Session::default()),
        AppId::Sensors => sensors(hub),
        AppId::Timer => timer(hub),
        AppId::Camera => camera(hub.camera),
        AppId::Lua(i) => lua_app(registry.apps.get(i as usize)),
    }
}

pub fn settings_pages(hub:&Hub,session:&crate::hub::settings::Session)->View<AppMsg>{super::settings_view::view(hub,session)}

pub fn camera(s:crate::camera::Snapshot)->View<AppMsg> {
    let live=s.state==crate::camera::State::Preview;
    let status=if s.error.is_some(){"相机未就绪，请检查左侧插槽或重试".into()}else if live{format!("640×480 · 已保存 {}",s.saved)}else{"正在连接左侧相机…".into()};
    page(AppId::Camera.key(),"相机",rs_lvgl_ui::obj().key(Key::Named("viewfinder"))
        .scrollable(false).style(Style::neutral().bg(CARD).width(Size::Px(360)).height(Size::Px(270)).align(Align::TopMid).offset(0,96))
        .child(text(if live{""}else if s.error.is_some(){"相机未就绪"}else{"正在启动"},font_24(),MUTED).style(Style::default().align(Align::Center))))
        .child(text(status,font_20(),MUTED).key(Key::Named("capture-status")).style(Style::default().align(Align::TopMid).offset(0,68)))
        .child(rs_lvgl_ui::button(if live{"拍照"}else{"重试"}).key(Key::Named("shutter"))
            .style(Style::neutral().width(Size::Px(144)).height(Size::Px(44)).bg(ACCENT).fg(TEXT).font(font_24()).radius(22).align(Align::TopMid).offset(0,372))
            .on_click(if live{AppMsg::CameraCapture}else{AppMsg::CameraRetry}))
}

/// Confirmation is keyed separately from the grid. Identity is captured in
/// tile events; neither dialog nor execution relies on a registry index.
pub fn launcher_managed(
    registry: &Registry,
    confirmation: Option<crate::hub::uninstall::Identity>,
    busy: bool,
    toast: Option<&str>,
) -> View<AppMsg> {
    let mut root = launcher(registry);
    if let Some(identity) = confirmation {
        let name = registry
            .find(identity.id())
            .map(|i| registry.apps[i].title.as_str())
            .unwrap_or(identity.id());
        let name = fit_text(name, 232);
        let prompt = if identity.origin == crate::hub::registry::Origin::Data {
            alloc::format!("卸载 {name}？\n应用和数据将删除。\n如有出厂版本，将恢复显示。")
        } else {
            alloc::format!("清除 {name} 数据？\n出厂应用将保留。")
        };
        let dialog = rs_lvgl_ui::column([])
            .key(Key::Named("manage-dialog"))
            .clickable(true)
            .scrollable(false)
            .style(
                card()
                    .width(Size::Px(360))
                    .height(Size::Content)
                    .align(Align::Center)
                    .gap(20),
            )
            .child(text(prompt, font_24(), TEXT).style(Style::default().width(Size::Px(320))))
            .child(
                rs_lvgl_ui::obj()
                    .clickable(true)
                    .scrollable(false)
                    .style(card().width(Size::Px(300)).height(Size::Px(56)))
                    .key(Key::Named("manage-confirm"))
                    .on_click(AppMsg::ManageConfirm)
                    .child(
                        text("确认", font_24(), TEXT)
                            .style(Style::default().align(Align::Center))
                            .clickable(false),
                    ),
            )
            .child(
                rs_lvgl_ui::obj()
                    .clickable(true)
                    .scrollable(false)
                    .style(card().width(Size::Px(300)).height(Size::Px(56)))
                    .key(Key::Named("manage-cancel"))
                    .on_click(AppMsg::ManageCancel)
                    .child(
                        text("取消", font_24(), TEXT)
                            .style(Style::default().align(Align::Center))
                            .clickable(false),
                    ),
            );
        root = root.child(
            rs_lvgl_ui::obj()
                .key(Key::Named("manage-overlay"))
                .clickable(true)
                .scrollable(false)
                .on_click(AppMsg::ManageCancel)
                .style(
                    Style::neutral()
                        .bg(BG)
                        .width(Size::Percent(100))
                        .height(Size::Percent(100))
                        .align(Align::Center),
                )
                .child(dialog),
        );
    } else if busy || toast.is_some() {
        root = root.child(
            text(
                if busy {
                    "正在处理…"
                } else {
                    toast.unwrap_or("")
                },
                font_24(),
                TEXT,
            )
            .key(Key::Named("manage-toast"))
            .style(card().align(Align::BottomMid).width(Size::Percent(100))),
        );
    }
    root
}

pub const REMOVED_TOAST: &str = "应用已卸载";
pub const CLEARED_TOAST: &str = "应用数据已清除";

pub const SOURCE_CHANGED_TOAST: &str = "应用来源已变化，请重新确认";
