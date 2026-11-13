//! Exactly one foreground task is selected by the OS engine feature.
#[cfg(feature = "lua-engine")]
pub(crate) use super::lua::task::lua_task as app_task;
#[cfg(feature = "lua-engine")]
pub(crate) use super::lua::task::*;
#[cfg(feature = "ts-engine")]
pub(crate) use super::ts::device::*;
