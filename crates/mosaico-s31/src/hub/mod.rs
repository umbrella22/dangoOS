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

use countdown::Countdown;
use gesture::SystemGesture;
use snapshot::SystemSnapshot;

/// The native apps, in launcher order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppId {
    Settings,
    Sensors,
    Timer,
}

impl AppId {
    pub const ALL: [AppId; 3] = [AppId::Settings, AppId::Sensors, AppId::Timer];

    /// Name under the launcher icon and in the page title.
    pub const fn title(self) -> &'static str {
        match self {
            AppId::Settings => "设置",
            AppId::Sensors => "传感器",
            AppId::Timer => "计时器",
        }
    }

    /// Stable key for the reconciler.
    pub const fn key(self) -> &'static str {
        match self {
            AppId::Settings => "app-settings",
            AppId::Sensors => "app-sensors",
            AppId::Timer => "app-timer",
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
    fn up_toggles_home_and_launcher_and_leaves_apps() {
        assert_eq!(Page::Home.after(SystemGesture::Up), Page::Launcher);
        assert_eq!(Page::Launcher.after(SystemGesture::Up), Page::Home);
        assert_eq!(Page::App(AppId::Timer).after(SystemGesture::Up), Page::Home);
    }

    #[test]
    fn back_walks_up_one_level() {
        assert_eq!(Page::App(AppId::Settings).after(SystemGesture::Back), Page::Launcher);
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
}
