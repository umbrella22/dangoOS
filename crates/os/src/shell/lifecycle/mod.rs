//! Page lifetime decisions. Resource stop requests and completion are distinct.
use crate::shell::navigation::route::{AppId, Page};
pub const fn is_lua_page(page: Page) -> bool {
    matches!(page, Page::App(AppId::Lua(_)))
}
#[cfg(target_os = "none")]
pub(crate) mod device;
