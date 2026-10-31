//! Logical routes. Lua identities are stable across registry rescans; indices
//! are resolved only when constructing a launcher or foreground page.
use super::gesture::SystemGesture;
use crate::apps::{lifecycle::uninstall::Identity, registry::Registry};
use crate::shell::pages::settings::model::Page as SettingsPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Home,
    Launcher,
    Settings(SettingsPage),
    Sensors,
    Timer,
    Camera,
    Lua(Identity),
    UnavailableLua,
}
impl Route {
    pub fn page(self, registry: &Registry) -> Page {
        match self {
            Self::Home => Page::Home,
            Self::Launcher => Page::Launcher,
            Self::Settings(_) => Page::App(AppId::Settings),
            Self::Sensors => Page::App(AppId::Sensors),
            Self::Timer => Page::App(AppId::Timer),
            Self::Camera => Page::App(AppId::Camera),
            // Running/reload follows the stable id, including a valid data
            // override. Launcher and management check the captured origin.
            Self::Lua(identity) => Page::App(AppId::Lua(
                registry
                    .find(identity.id())
                    .map_or(u8::MAX, |index| index as u8),
            )),
            Self::UnavailableLua => Page::App(AppId::Lua(u8::MAX)),
        }
    }
    pub fn after(self, gesture: SystemGesture) -> Self {
        match (self, gesture) {
            (Self::Home, SystemGesture::Up) => Self::Launcher,
            (_, SystemGesture::Up) => Self::Home,
            (Self::Home, SystemGesture::Back) => Self::Home,
            (Self::Launcher, SystemGesture::Back) => Self::Home,
            (Self::Settings(page), SystemGesture::Back) if page != SettingsPage::Root => {
                Self::Settings(page.back())
            }
            (_, SystemGesture::Back) => Self::Launcher,
        }
    }
}

/// The apps: the native ones, then the Lua apps of the [`registry`] by their
/// index there (the launcher lists them in that order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppId {
    Settings,
    Sensors,
    Timer,
    Camera,
    Lua(u8),
}

impl AppId {
    pub const NATIVE: [AppId; 4] = [AppId::Settings, AppId::Sensors, AppId::Timer, AppId::Camera];

    /// Name of a native app under its icon and in the page title.
    pub const fn title(self) -> &'static str {
        match self {
            AppId::Settings => "设置",
            AppId::Sensors => "传感器",
            AppId::Timer => "计时器",
            AppId::Camera => "相机",
            AppId::Lua(_) => "Lua",
        }
    }

    /// Stable key of a native app's page for the reconciler.
    pub const fn key(self) -> &'static str {
        match self {
            AppId::Settings => "app-settings",
            AppId::Sensors => "app-sensors",
            AppId::Timer => "app-timer",
            AppId::Camera => "app-camera",
            AppId::Lua(_) => "app-lua",
        }
    }

    /// The registry index, for Lua apps.
    pub const fn lua_index(self) -> Option<usize> {
        match self {
            AppId::Lua(i) => Some(i as usize),
            _ => None,
        }
    }
}

/// Which page is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The fox.
    Home,
    /// The app grid.
    Launcher,
    App(AppId),
}

impl Page {
    /// Where a system gesture leads from this page.
    pub fn after(self, gesture: SystemGesture) -> Page {
        match (self, gesture) {
            (Page::Home, SystemGesture::Up) => Page::Launcher,
            (_, SystemGesture::Up) => Page::Home,
            (Page::App(_), SystemGesture::Back) => Page::Launcher,
            (Page::Launcher, SystemGesture::Back) => Page::Home,
            (Page::Home, SystemGesture::Back) => Page::Home,
        }
    }

    /// Whether LVGL draws this page (everything but the fox).
    pub fn is_lvgl(self) -> bool {
        self != Page::Home
    }
}
