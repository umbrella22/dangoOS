//! Placeholder for a foreground Lua application.
use alloc::string::String;

use rs_lvgl_ui::{Align, Color, Key, Size, Style, View};

use crate::apps::registry::AppEntry;
use crate::shell::messages::AppMsg;

use crate::shell::components::page::*;
use crate::shell::pages::launcher::lua_key;
use crate::shell::theme::tokens::*;
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
