//! Native pages and their view dispatch.
use crate::apps::registry::Registry;
use crate::shell::app::AppMsg;
use crate::shell::model::Hub;
use crate::shell::navigation::route::AppId;
use rs_lvgl_ui::View;
pub mod camera;
pub mod launcher;
pub mod low_battery;
pub mod lua;
pub mod sensors;
pub mod settings;
pub mod timer;
/// The view for one app.
pub fn app(hub: &Hub, registry: &Registry, app: AppId) -> View<AppMsg> {
    match app {
        AppId::Settings => settings::view::view(
            hub,
            &crate::shell::pages::settings::model::Session::default(),
            settings::model::Page::Root,
        ),
        AppId::Sensors => sensors::sensors(hub),
        AppId::Timer => timer::timer(hub),
        AppId::Camera => camera::camera(hub.camera),
        AppId::Lua(i) => lua::lua_app(registry.apps.get(i as usize)),
    }
}
