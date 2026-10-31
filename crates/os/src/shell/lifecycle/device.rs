//! Page enter/leave effects. Stop requests release queued frames immediately;
//! Lua's worker publishes completion after VM drop and before starting a queued
//! replacement. Display leases retain pixels through their final upload.
use super::is_lua_page;
use crate::apps::lua::task as lua;
use crate::companion::{render::entrance, runtime::Runtime};
use crate::services::{
    display::lvgl_port::{self, DisplayPort},
    feedback::lights,
};
use crate::shell::runtime::frames::{Frames, Slide};
use crate::shell::{
    app::MosaicoApp,
    navigation::{
        gesture::EdgeGestures,
        route::{AppId, Page},
        transition,
    },
};
use log::{info, warn};
/// Who drives the LEDs on a page: the fox on Home, the Settings preview in
/// Settings (so the light switch shows on the hardware), dark elsewhere.
fn light_owner(page: Page) -> lights::Owner {
    match page {
        Page::Home => lights::Owner::Fox,
        Page::App(AppId::Settings) => lights::Owner::Preview,
        Page::App(AppId::Lua(_)) => lights::Owner::Lua,
        _ => lights::Owner::Dark,
    }
}

/// Start the Lua app on `page`, if it is one that can run.
pub(crate) fn start_lua_page(app: &MosaicoApp, page: Page) {
    match app.lua_entry(page) {
        Some(entry) if entry.runnable() => lua::start(entry.app.clone()),
        Some(entry) => warn!("apps: {} needs {:?}", entry.app.id, entry.missing),
        None => {}
    }
}

pub(crate) fn apply(
    from: Page,
    to: Page,
    app: &MosaicoApp,
    gestures: &mut EdgeGestures,
    runtime: &mut Runtime,
    port: &mut DisplayPort,
    frames: &mut Frames,
    now_ms: u64,
) {
    info!("hub: {:?} -> {:?}", from, to);
    gestures.reset_pass();
    gestures.set_full_screen(is_lua_page(to));
    lvgl_port::set_pointer(0, 0, false);
    if is_lua_page(from) {
        lua::stop();
        lua::set_pointer(0, 0, false);
    }
    start_lua_page(app, to);
    if from == Page::App(AppId::Camera) || to == Page::App(AppId::Camera) {
        crate::services::camera::task::set_ui_preview(to == Page::App(AppId::Camera));
        frames.last_camera_frame = 0;
    }
    if to == Page::Home {
        // The entrance replays; a finger still on the glass from the
        // swipe must not interrupt it.
        runtime.ignore_touch_until(now_ms + entrance::TOTAL_MS);
        port.discard_damage();
        frames.clear_on_fox = true;
    } else {
        port.invalidate_all();
    }
    frames.slide = match transition::Direction::between(from, to) {
        Some(direction) => Slide::Pending(direction),
        None => Slide::Idle,
    };
    lights::set_owner(light_owner(to));
}
