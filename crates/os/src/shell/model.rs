//! The hub shell (RFC 0006): navigation between the fox home page, the app
//! launcher and full-screen apps, plus the state the apps keep while they are
//! not on screen.
//!
//! Everything here is plain state and logic, host tested. The LVGL views live
//! in [`crate::shell::pages::native`]; the device task routes touches through
//! [`gesture::EdgeGestures`] and applies [`Settings`] to the hardware.

use crate::services::input::snapshot::SystemSnapshot;
use crate::services::settings::preferences::Settings;
use crate::services::timer::Countdown;

/// Whether a sensor event still reaches the fox while an app is open.
///
/// Off Home the fox is paused: the apps own the lights and the motor, and a
/// knock or a key press must not make her react behind the user's back. Only
/// events that change lasting state go through, so she is not out of date
/// when Home comes back: the battery, modules coming and going, voice
/// activity and the releases that end a held key or magnet contact.
pub fn reaches_fox_off_home(kind: &crate::companion::behavior::affect::EventKind) -> bool {
    use crate::companion::behavior::affect::EventKind;
    matches!(
        kind,
        EventKind::BatteryLevel(_)
            | EventKind::ModuleInserted(_)
            | EventKind::ModuleRemoved(_)
            | EventKind::Activity(_)
            | EventKind::NetworkConnection(_)
            | EventKind::BluetoothConnection(_)
            | EventKind::MusicPlayback(_)
            | EventKind::KeyUp
            | EventKind::MagneticRelease
    )
}

pub use crate::apps::registry::LuaApp;

/// Read-only facts for the "about" row, refreshed by the device task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct About {
    pub uptime_s: u64,
    /// Most recent service sample, KiB (internal + PSRAM); absent until sampled.
    pub free_kib: Option<u32>,
}

/// Hub state: navigation plus every app's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hub {
    pub settings: Settings,
    pub snapshot: SystemSnapshot,
    pub countdown: Countdown,
    pub about: About,
    pub camera: crate::services::camera::Snapshot,
    pub radio: crate::services::radio::Snapshot,
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
            settings: Settings::new(),
            snapshot: SystemSnapshot::new(),
            countdown: Countdown::new(),
            camera: crate::services::camera::Snapshot::new(),
            radio: crate::services::radio::Snapshot::new(),
            about: About {
                uptime_s: 0,
                free_kib: None,
            },
            now_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::navigation::gesture::SystemGesture;
    use crate::shell::navigation::route::{AppId, Page};

    #[test]
    fn off_home_only_lasting_state_reaches_the_fox() {
        use {
            crate::companion::behavior::affect::EventKind,
            crate::companion::behavior::affect::ModuleKind,
        };
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
