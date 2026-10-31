//! The Lua app registry: `launcher.json` manifests found under the app
//! roots, with the rules of esp-claw's `app_registry` (`parse_manifest`,
//! `load_root`).
//!
//! Roots, in priority order (an id found in an earlier root shadows the same
//! id later on, as the official registry does):
//!
//! 1. `/data/apps`: apps pushed over USB (`@mosaico` protocol).
//! 2. `/system/apps`: the factory apps on the official read-only partition.
//!
//! A manifest is accepted when `schema_version` is 1, `id` is valid and
//! equals the directory name, `entry` is a relative `.lua` path to an existing
//! file, `icon` (optional) is a relative `.jpg`/`.jpeg` path, `display_name`
//! (optional) is a non-empty string, `args` (optional) is an object, `order`
//! (optional) is an integer and `visible` (optional) a boolean. Invisible apps
//! are skipped, like the official launcher does.
//!
//! On top of that the entry script is checked for modules this firmware does
//! not have (for example network-only modules): such apps are listed but marked
//! unavailable instead of failing on start.

mod icons;
mod manifest;
mod modules;
mod scan;
#[cfg(test)]
mod tests;

use alloc::{string::String, vec::Vec};
#[cfg(test)]
use icons::scale_to_box;
pub use icons::{Icon, IconCache};
pub use manifest::{id_is_valid, parse_manifest, relative_path_is_valid, Manifest};
pub use modules::missing_modules;
pub(crate) use scan::read_text;

/// `launcher.json`, as the official registry names it.
pub const MANIFEST: &str = "launcher.json";
/// The official limit on manifest size.
pub const MANIFEST_MAX_BYTES: u32 = 4096;
/// The official limit on registry entries.
pub const MAX_APPS: usize = 64;
/// Longest app id (`APP_REGISTRY_ID_MAX_LEN`).
pub const ID_MAX_LEN: usize = 63;
/// The app roots on the file system, highest priority first.
pub const ROOTS: [(&str, Origin); 2] = [
    ("/data/apps", Origin::Data),
    ("/system/apps", Origin::System),
];
/// Directories `require` searches after the app's own (esp-claw
/// `app_capabilities.c` adds `<system>/scripts/builtin` and its `lib`).
pub const SHARED_LUA_DIRS: [&str; 2] = ["/system/scripts/builtin", "/system/scripts/builtin/lib"];
/// Lua script size checked for module use (larger scripts are checked in
/// their first part only).
const SCAN_MAX_BYTES: u32 = 256 * 1024;
/// Side of the launcher icon box, px.
pub const ICON_BOX: u16 = 96;

/// Modules every app gets: the Lua standard libraries and the esp-claw
/// modules `lua-engine` provides.
pub const PROVIDED_MODULES: [&str; 25] = [
    "string",
    "table",
    "math",
    "io",
    "os",
    "coroutine",
    "utf8",
    "debug",
    "package",
    "display",
    "image",
    "lvgl",
    "thread",
    "delay",
    "system",
    "json",
    "arg_schema",
    "storage",
    "imu",
    "magnetometer",
    "fuel_gauge",
    "lib_fuel_gauge",
    "haptic",
    "led_strip",
    "button",
];

/// Where an app comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Data,
    System,
}

/// What the Lua task runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaApp {
    pub id: String,
    /// Absolute VFS path to the application entry script.
    pub entry: String,
    /// `args` as JSON text (`{}` when absent).
    pub args_json: String,
}

/// One registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    pub app: LuaApp,
    pub title: String,
    pub order: i32,
    pub origin: Origin,
    /// Absolute application directory on the selected mount.
    pub dir: String,
    pub icon: Option<Icon>,
    /// Modules the entry requires that this firmware lacks. Non-empty means
    /// the app is listed but cannot run.
    pub missing: Vec<String>,
}

impl AppEntry {
    pub fn runnable(&self) -> bool {
        self.missing.is_empty()
    }
}

/// The scanned apps, in launcher order: runnable ones first, then by
/// `order` and title as the official launcher sorts them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub apps: Vec<AppEntry>,
}
