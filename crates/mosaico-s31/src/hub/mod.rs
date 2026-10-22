//! The hub shell (RFC 0006): navigation between the fox home page, the app
//! launcher and full-screen apps, plus the state the apps keep while they are
//! not on screen.
//!
//! Everything here is plain state and logic, host tested. The LVGL views live
//! in [`crate::ui::hub_view`]; the device task routes touches through
//! [`gesture::EdgeGestures`] and applies [`Settings`] to the hardware.

pub mod countdown;
pub mod gesture;
pub(crate) mod json;
pub mod push;
pub mod registry;
pub mod skins;
pub mod snapshot;
pub mod transition;

use countdown::Countdown;
use gesture::SystemGesture;
use snapshot::SystemSnapshot;

/// Whether a sensor event still reaches the fox while an app is open.
///
/// Off Home the fox is paused: the apps own the lights and the motor, and a
/// knock or a key press must not make her react behind the user's back. Only
/// events that change lasting state go through, so she is not out of date
/// when Home comes back: the battery, modules coming and going, voice
/// activity and the releases that end a held key or magnet contact.
pub fn reaches_fox_off_home(kind: &crate::behavior::affect::EventKind) -> bool {
    use crate::behavior::affect::EventKind;
    matches!(
        kind,
        EventKind::BatteryLevel(_)
            | EventKind::ModuleInserted(_)
            | EventKind::ModuleRemoved(_)
            | EventKind::Activity(_)
            | EventKind::KeyUp
            | EventKind::MagneticRelease
    )
}

pub use registry::{LuaApp, LuaSource};

/// The apps: the native ones, then the Lua apps of the [`registry`] by their
/// index there (the launcher lists them in that order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppId {
    Settings,
    Sensors,
    Timer,
    Lua(u8),
}

impl AppId {
    pub const NATIVE: [AppId; 3] = [AppId::Settings, AppId::Sensors, AppId::Timer];

    /// Name of a native app under its icon and in the page title.
    pub const fn title(self) -> &'static str {
        match self {
            AppId::Settings => "设置",
            AppId::Sensors => "传感器",
            AppId::Timer => "计时器",
            AppId::Lua(_) => "Lua",
        }
    }

    /// Stable key of a native app's page for the reconciler.
    pub const fn key(self) -> &'static str {
        match self {
            AppId::Settings => "app-settings",
            AppId::Sensors => "app-sensors",
            AppId::Timer => "app-timer",
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

/// User settings. Not persisted yet: a reboot restores the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Panel brightness, percent `MIN_BRIGHTNESS..=100`.
    pub brightness: u8,
    /// The interaction-board lights.
    pub lights: bool,
    /// The vibration motor.
    pub haptics: bool,
}

impl Settings {
    pub const MIN_BRIGHTNESS: u8 = 10;
    /// The boot brightness the plane task already uses.
    pub const DEFAULT_BRIGHTNESS: u8 = 100;

    pub const fn new() -> Self {
        Self {
            brightness: Self::DEFAULT_BRIGHTNESS,
            lights: true,
            haptics: true,
        }
    }

    pub fn set_brightness(&mut self, percent: i32) {
        self.brightness = percent.clamp(Self::MIN_BRIGHTNESS as i32, 100) as u8;
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

/// Read-only facts for the "about" row, refreshed by the device task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub uptime_s: u64,
    /// Free heap, KiB (internal + PSRAM).
    pub free_kib: u32,
}

/// Hub state: navigation plus every app's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hub {
    pub page: Page,
    pub settings: Settings,
    pub snapshot: SystemSnapshot,
    pub countdown: Countdown,
    pub about: About,
    /// The clock the views read, ms (same clock as [`Countdown`]).
    pub now_ms: u64,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub const fn new() -> Self {
        Self {
            page: Page::Home,
            settings: Settings::new(),
            snapshot: SystemSnapshot::new(),
            countdown: Countdown::new(),
            about: About {
                uptime_s: 0,
                free_kib: 0,
            },
            now_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_home_only_lasting_state_reaches_the_fox() {
        use crate::behavior::affect::{EventKind, ModuleKind};
        for kind in [
            EventKind::TouchPoke,
            EventKind::TouchPet,
            EventKind::KeyDown,
            EventKind::LightLevel(80),
            EventKind::Presence(true),
            EventKind::MagneticContact,
            EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg: 2_500,
            },
        ] {
            assert!(!reaches_fox_off_home(&kind), "{kind:?}");
        }
        for kind in [
            EventKind::BatteryLevel(10),
            EventKind::ModuleRemoved(ModuleKind::Interaction),
            EventKind::KeyUp,
        ] {
            assert!(reaches_fox_off_home(&kind), "{kind:?}");
        }
    }

    #[test]
    fn up_toggles_home_and_launcher_and_leaves_apps() {
        assert_eq!(Page::Home.after(SystemGesture::Up), Page::Launcher);
        assert_eq!(Page::Launcher.after(SystemGesture::Up), Page::Home);
        assert_eq!(Page::App(AppId::Timer).after(SystemGesture::Up), Page::Home);
    }

    #[test]
    fn back_walks_up_one_level() {
        assert_eq!(
            Page::App(AppId::Settings).after(SystemGesture::Back),
            Page::Launcher
        );
        assert_eq!(Page::Launcher.after(SystemGesture::Back), Page::Home);
        assert_eq!(Page::Home.after(SystemGesture::Back), Page::Home);
    }

    #[test]
    fn brightness_is_clamped() {
        let mut s = Settings::new();
        s.set_brightness(0);
        assert_eq!(s.brightness, Settings::MIN_BRIGHTNESS);
        s.set_brightness(250);
        assert_eq!(s.brightness, 100);
        s.set_brightness(55);
        assert_eq!(s.brightness, 55);
    }

    #[test]
    fn only_home_is_drawn_by_the_fox() {
        assert!(!Page::Home.is_lvgl());
        assert!(Page::Launcher.is_lvgl());
        assert!(Page::App(AppId::Sensors).is_lvgl());
    }

    #[test]
    fn lua_apps_are_registry_indices() {
        assert_eq!(AppId::Lua(3).lua_index(), Some(3));
        assert_eq!(AppId::Timer.lua_index(), None);
        assert!(Page::App(AppId::Lua(0)).is_lvgl());
        assert_eq!(
            Page::App(AppId::Lua(1)).after(SystemGesture::Back),
            Page::Launcher
        );
    }
}
