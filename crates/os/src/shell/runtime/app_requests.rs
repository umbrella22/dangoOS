//! Registry publication and foreground application requests.
use super::*;
use crate::shell::lifecycle::device::start_lua_page;
/// Apply a request from the USB push protocol (see `apps`).
pub(super) fn apply_apps_request(
    app: &mut MosaicoApp,
    req: apps::Request,
    registry: Option<crate::apps::registry::Registry>,
) {
    let page = app.page();
    let open_id = app.lua_entry(page).map(|e| e.app.id.clone());
    if req.reload {
        app.set_registry(registry.expect("rescan result precedes UI publication"));
        if is_lua_page(page) {
            // The same page, possibly at a new index: restart it with its
            // new files, or stop it if it is gone or cannot run any more.
            let still = app
                .lua_entry(app.page())
                .filter(|e| e.runnable())
                .map(|e| e.app.clone());
            match (still, &req.action) {
                (_, Some(apps::Action::Stop)) => {}
                (Some(entry), _) if open_id.as_deref() == Some(entry.id.as_str()) => {
                    info!("apps: restarting {}", entry.id);
                    lua::start(entry);
                }
                _ => lua::stop(),
            }
        }
    }
    match req.action {
        Some(apps::Action::Run(id)) => match app.registry().find(&id) {
            Some(i) => {
                let target = Page::App(AppId::Lua(i as u8));
                if app.page() == target {
                    if !req.reload {
                        start_lua_page(app, target);
                    }
                } else {
                    // The page change starts it (see the main loop).
                    app.go(target);
                }
            }
            None => warn!("apps: run {id}: no such app"),
        },
        Some(apps::Action::Stop) => {
            if is_lua_page(app.page()) {
                app.go(Page::Launcher);
            }
        }
        None => {}
    }
}
