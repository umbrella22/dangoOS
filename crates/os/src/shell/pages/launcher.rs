//! Launcher grid and management confirmation.
use alloc::string::String;
use alloc::vec::Vec;

use rs_lvgl_ui::{symbol, Align, Color, FlexAlign, ImageSource, Key, Size, Style, View};

use crate::apps::registry::Registry;
use crate::shell::messages::AppMsg;
use crate::shell::navigation::route::AppId;

use crate::shell::components::page::*;
use crate::shell::theme::tokens::*;
const LETTER_COLORS: [Color; 4] = [
    Color::rgb(0x2E, 0x9E, 0x5B),
    Color::rgb(0xC8, 0x8A, 0x2E),
    Color::rgb(0x8A, 0x4F, 0xD8),
    Color::rgb(0x2E, 0x8E, 0xA8),
];

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
    Tile {
        app: AppId::Camera,
        icon: symbol::IMAGE,
        color: GREEN,
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
    Icon(&'a crate::apps::registry::Icon),
    /// A Lua app without an icon: the first letter of its name.
    Letter(String, Color),
}

fn tile(
    key: Key,
    app: AppId,
    face: TileFace,
    name: &str,
    dim: bool,
    identity: Option<crate::apps::lifecycle::uninstall::Identity>,
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
            crate::apps::lifecycle::uninstall::Identity::new(&entry.app.id, entry.origin),
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

/// Confirmation is keyed separately from the grid. Identity is captured in
/// tile events; neither dialog nor execution relies on a registry index.
pub fn launcher_managed(
    registry: &Registry,
    confirmation: Option<crate::apps::lifecycle::uninstall::Identity>,
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
        let prompt = if identity.origin == crate::apps::registry::Origin::Data {
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
