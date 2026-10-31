//! Settings subpage dispatch with shared frame composition.
mod audio;
mod camera;
mod categories;
mod character;
mod components;
mod display;
mod feedback;
mod password;
mod proxy;
mod storage;
mod system;
mod update;
mod wifi;
mod wireless;

use crate::shell::{
    messages::AppMsg,
    model::Hub,
    pages::settings::model::{Page, Session},
};
use rs_lvgl_ui::View;

pub fn view(hub: &Hub, session: &Session, settings_page: Page) -> View<AppMsg> {
    let body = match settings_page {
        Page::Root => categories::view(hub, session),
        Page::Character => character::view(session),
        Page::Display => display::view(&hub.settings),
        Page::Audio => audio::view(&hub.settings, session.info),
        Page::Feedback => feedback::view(&hub.settings),
        Page::Wireless => wireless::view(session),
        Page::Wifi => wifi::view(session),
        Page::KnownNetwork => wifi::known_network(session),
        Page::Password => return password::view(session),
        Page::Camera => camera::view(&hub.settings),
        Page::Storage => storage::view(session.info),
        Page::Update => update::view(hub, session),
        Page::Proxy => return proxy::view(session),
        Page::System => system::view(hub, session.info),
    };
    components::frame(body, settings_page, session.info)
}
