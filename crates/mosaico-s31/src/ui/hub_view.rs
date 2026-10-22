//! LVGL views for the hub pages (RFC 0006): the launcher and the three native
//! apps. The fox home page stays in [`super::fox_widget`].
//!
//! Layout rules for the 480x480 round-cornered panel:
//!
//! - nothing interactive below [`gesture::BOTTOM_BAND_Y`] or left of
//!   [`gesture::LEFT_BAND_X`]: those bands belong to the system gestures;
//! - content stays inside a 60 px corner safe area, so pages are centred
//!   columns at most 400 px wide;
//! - colours and sizes follow the legacy design tokens
//!   (`docs/legacy/MOSAICO_UI_DESIGN.md`): black field, 30 px titles, 24 px
//!   labels, 22 px values, orange accent.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use lvgl_ui::{
    symbol, Align, Color, FlexAlign, Font, GridTrack, ImageSource, Key, Node, Part, Size, State,
    Style, TextAlign,
};

use super::hub_font::{HUB_20, HUB_24, HUB_30, HUB_DIGITS_64};
use super::AppMsg;
use crate::behavior::affect::ModuleKind;
use crate::hub::countdown::{format_mm_ss, Phase, PRESETS_MIN};
use crate::hub::registry::{AppEntry, Registry};
use crate::hub::snapshot::{MagnetState, SlotState, SystemSnapshot};
use crate::hub::{AppId, Hub, Settings};

/// Design tokens.
pub mod tokens {
    use lvgl_ui::Color;

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

fn font_20() -> Font {
    HUB_20.font()
}
fn font_24() -> Font {
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
    unsafe { Font::from_raw(core::ptr::addr_of!(lvgl::sys::lv_font_montserrat_48)) }
}

fn text(content: impl Into<String>, font: Font, color: Color) -> Node<AppMsg> {
    Node::label(content).style(Style::neutral().font(font).fg(color))
}

/// A full-screen page: black field, centred title, home indicator.
fn page(key: &'static str, title: &'static str, body: Node<AppMsg>) -> Node<AppMsg> {
    Node::obj()
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
fn home_indicator() -> Node<AppMsg> {
    Node::obj().key(Key::Named("home-pill")).style(
        Style::neutral()
            .bg(WEAK)
            .radius(3)
            .width(Size::Px(96))
            .height(Size::Px(6))
            .align(Align::BottomMid)
            .offset(0, -16),
    )
}

/// The content column under the title.
fn content(key: &'static str) -> Node<AppMsg> {
    Node::column().key(Key::Named(key)).scrollable(false).style(
        Style::neutral()
            .width(Size::Px(COLUMN_W))
            .height(Size::Content)
            .gap(8)
            .align(Align::TopMid)
            .offset(0, CONTENT_Y),
    )
}

fn card() -> Style {
    Style::neutral().bg(CARD).radius(RADIUS).pad(20)
}

/// A 68 px row: label on the left, `trailing` on the right.
fn row(key: &'static str, label: &'static str, trailing: Node<AppMsg>) -> Node<AppMsg> {
    Node::row()
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

fn switch(key: &'static str, on: bool, msg: fn(bool) -> AppMsg) -> Node<AppMsg> {
    Node::switch(on)
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

const NATIVE_TILES: [Tile; 3] = [
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

fn tile(key: Key, app: AppId, face: TileFace, name: &str, dim: bool) -> Node<AppMsg> {
    let square = Node::obj()
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
            Node::image(ImageSource::Rgb565 {
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
    Node::column()
        .key(key)
        .scrollable(false)
        .clickable(true)
        .on_click(AppMsg::Open(app))
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
    const MAX: u32 = 120;
    let w = |c: char| super::hub_font::advance(&HUB_24, c).unwrap_or(24) as u32;
    if name.chars().map(w).sum::<u32>() <= MAX {
        return name.into();
    }
    let mut out = String::new();
    let mut used = w('…');
    for c in name.chars() {
        if used + w(c) > MAX {
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
pub fn launcher(registry: &Registry) -> Node<AppMsg> {
    let mut tiles: Vec<Node<AppMsg>> = NATIVE_TILES
        .iter()
        .map(|t| {
            tile(
                Key::Named(t.app.key()),
                t.app,
                TileFace::Symbol(t.icon, t.color),
                t.app.title(),
                false,
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
        ));
    }
    let row_count = tiles.len().div_ceil(TILES_PER_ROW);
    let mut rows = Vec::with_capacity(row_count);
    let mut tiles = tiles.into_iter();
    for r in 0..row_count {
        rows.push(
            Node::row()
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
    let grid = Node::column().key(Key::Named("tiles"));
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

    Node::obj()
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
pub fn lua_app(entry: Option<&AppEntry>) -> Node<AppMsg> {
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
    Node::obj()
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

// ---------------------------------------------------------------- settings

/// Settings: brightness, lights, haptics, about.
pub fn settings(settings: &Settings, uptime_s: u64, free_kib: u32) -> Node<AppMsg> {
    let brightness = Node::column()
        .key(Key::Named("brightness"))
        .scrollable(false)
        .style(
            card()
                .width(Size::Percent(100))
                .height(Size::Content)
                .gap(18),
        )
        .child(
            Node::row()
                .key(Key::Named("head"))
                .scrollable(false)
                .style(
                    Style::neutral()
                        .width(Size::Percent(100))
                        .height(Size::Content)
                        .flex_align(FlexAlign::SpaceBetween, FlexAlign::Center),
                )
                .child(text("亮度", font_24(), TEXT).key(Key::Named("label")))
                .child(
                    text(format!("{}%", settings.brightness), font_24(), MUTED)
                        .key(Key::Named("value")),
                ),
        )
        .child(
            Node::slider(
                Settings::MIN_BRIGHTNESS as i32,
                100,
                settings.brightness as i32,
            )
            .key(Key::Named("slider"))
            .style(
                Style::default()
                    .width(Size::Px(COLUMN_W - 40 - 16))
                    .height(Size::Px(10))
                    .offset(8, 0)
                    .bg(CARD_HI)
                    .radius(5),
            )
            .part_style(Part::Indicator, Style::default().bg(ACCENT).radius(5))
            .part_style(Part::Knob, Style::default().bg(TEXT).pad(8))
            .on_value_changed(AppMsg::Brightness),
        );

    let minutes = uptime_s / 60;
    let about = if minutes < 60 {
        format!("运行 {} 分 · {} KB", minutes, free_kib)
    } else {
        format!("运行 {} 时 {} 分", minutes / 60, minutes % 60)
    };

    page(
        AppId::Settings.key(),
        AppId::Settings.title(),
        content("rows")
            .child(brightness)
            .child(row(
                "lights",
                "灯光",
                switch("switch", settings.lights, AppMsg::Lights),
            ))
            .child(row(
                "haptics",
                "震动",
                switch("switch", settings.haptics, AppMsg::Haptics),
            ))
            .child(row(
                "about",
                "关于",
                text(about, font_20(), MUTED).key(Key::Named("value")),
            )),
    )
}

// ----------------------------------------------------------------- sensors

/// Three rows of cards end at the gesture band: 80 + 3 x 108 + 2 x 8 = 420.
const SENSOR_CARD_H: i16 = 108;
const SENSOR_Y: i16 = 80;

fn sensor_card(
    key: &'static str,
    title: &'static str,
    lines: [String; 2],
    bar: Option<(u8, Color)>,
) -> Node<AppMsg> {
    let mut card_node = Node::column()
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
                Node::bar(0, 100, value as i32)
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

/// Sensors: a 2x3 grid of live values.
pub fn sensors(snapshot: &SystemSnapshot) -> Node<AppMsg> {
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

    let cards = [
        sensor_card("tilt", "姿态", tilt, None),
        sensor_card("gyro", "角速度", gyro, None),
        sensor_card("light", "光线", light.0, light.1),
        sensor_card("battery", "电量", battery.0, battery.1),
        sensor_card("slots", "模块", slots, None),
        sensor_card(
            "presence",
            "状态",
            [String::from(presence), String::from(magnet)],
            None,
        ),
    ];
    let grid = Node::grid(
        [GridTrack::Px(196), GridTrack::Px(196)],
        [
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
            GridTrack::Px(SENSOR_CARD_H),
        ],
    )
    .key(Key::Named("grid"))
    .scrollable(false)
    .style(
        Style::neutral()
            .width(Size::Px(COLUMN_W))
            .height(Size::Content)
            .gap(8)
            .align(Align::TopMid)
            .offset(0, SENSOR_Y),
    )
    .children(
        cards
            .into_iter()
            .enumerate()
            .map(|(i, card)| card.grid_cell((i % 2) as u16, (i / 2) as u16, 1, 1)),
    );

    page(AppId::Sensors.key(), AppId::Sensors.title(), grid)
}

// ------------------------------------------------------------------- timer

fn chip(index: usize, minutes: u32, selected: bool, enabled: bool) -> Node<AppMsg> {
    let mut node = Node::obj()
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

fn pill_button(key: &'static str, label: &'static str, color: Color, msg: AppMsg) -> Node<AppMsg> {
    Node::obj()
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
pub fn timer(hub: &Hub) -> Node<AppMsg> {
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
    let ring = Node::obj()
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
            Node::arc(0, 1000, permille)
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
            Node::column()
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

    let chips = Node::row()
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

    let controls = Node::row()
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

    Node::obj()
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
pub fn app(hub: &Hub, registry: &Registry, app: AppId) -> Node<AppMsg> {
    match app {
        AppId::Settings => settings(&hub.settings, hub.about.uptime_s, hub.about.free_kib),
        AppId::Sensors => sensors(&hub.snapshot),
        AppId::Timer => timer(hub),
        AppId::Lua(i) => lua_app(registry.apps.get(i as usize)),
    }
}
