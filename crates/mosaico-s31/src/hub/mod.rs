//! The hub shell (RFC 0006): navigation between the fox home page, the app
//! launcher and full-screen apps, plus the state the apps keep while they are
//! not on screen.
//!
//! Everything here is plain state and logic, host tested. The LVGL views live
//! in [`crate::ui::hub_view`]; the device task routes touches through
//! [`gesture::EdgeGestures`] and applies [`Settings`] to the hardware.

pub mod countdown;
pub mod gesture;
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

/// The apps, in launcher order: the native ones, then the built-in Lua apps
/// (esp-claw-skills-lab scripts run unchanged by `mosaico-lua`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppId {
    Settings,
    Sensors,
    Timer,
    Snake,
    Game2048,
}

impl AppId {
    pub const ALL: [AppId; 5] = [
        AppId::Settings,
        AppId::Sensors,
        AppId::Timer,
        AppId::Snake,
        AppId::Game2048,
    ];

    /// Name under the launcher icon and in the page title.
    pub const fn title(self) -> &'static str {
        match self {
            AppId::Settings => "设置",
            AppId::Sensors => "传感器",
            AppId::Timer => "计时器",
            AppId::Snake => "贪吃蛇",
            AppId::Game2048 => "2048",
        }
    }

    /// Stable key for the reconciler.
    pub const fn key(self) -> &'static str {
        match self {
            AppId::Settings => "app-settings",
            AppId::Sensors => "app-sensors",
            AppId::Timer => "app-timer",
            AppId::Snake => "app-snake",
            AppId::Game2048 => "app-2048",
        }
    }

    /// The bundled Lua app, for apps that are scripts.
    pub fn lua(self) -> Option<LuaApp> {
        match self {
            AppId::Snake => Some(LuaApp::Snake),
            AppId::Game2048 => Some(LuaApp::Game2048),
            _ => None,
        }
    }
}

/// A Lua app bundled in the firmware: its `launcher.json` id and entry
/// script, copied unchanged from esp-claw-skills-lab (MIT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuaApp {
    Snake,
    Game2048,
}

impl LuaApp {
    /// `launcher.json` `id`.
    pub const fn id(self) -> &'static str {
        match self {
            LuaApp::Snake => "snake",
            LuaApp::Game2048 => "game_2048",
        }
    }

    /// Chunk name of the entry script (shown in Lua error messages).
    pub const fn chunk_name(self) -> &'static str {
        match self {
            LuaApp::Snake => "draw_snake.lua",
            LuaApp::Game2048 => "game_2048.lua",
        }
    }

    /// `launcher.json` `args`, as JSON.
    pub const fn args_json(self) -> &'static str {
        match self {
            LuaApp::Snake => "{}",
            LuaApp::Game2048 => r#"{"run_time_ms": 0}"#,
        }
    }

    pub const fn source(self) -> &'static str {
        match self {
            LuaApp::Snake => {
                include_str!("../../../mosaico-lua/vendor/skills-lab/snake/scripts/draw_snake.lua")
            }
            LuaApp::Game2048 => include_str!(
                "../../../mosaico-lua/vendor/skills-lab/game_2048/scripts/game_2048.lua"
            ),
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
    fn lua_apps_match_their_launcher_json() {
        for app in [LuaApp::Snake, LuaApp::Game2048] {
            let dir = match app {
                LuaApp::Snake => "snake",
                LuaApp::Game2048 => "game_2048",
            };
            let path = std::format!(
                "{}/../mosaico-lua/vendor/skills-lab/{dir}/launcher.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let json = std::fs::read_to_string(path).unwrap();
            assert!(
                json.contains(&std::format!("\"id\": \"{}\"", app.id())),
                "{json}"
            );
            let entry = std::format!("scripts/{}", app.chunk_name());
            assert!(json.contains(&entry), "{json}");
            assert!(!app.source().is_empty());
        }
        assert_eq!(AppId::Snake.lua(), Some(LuaApp::Snake));
        assert_eq!(AppId::Timer.lua(), None);
    }
}
