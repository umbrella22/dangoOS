//! services / settings functionality.

#[cfg(target_os = "none")]
pub(crate) mod device;
pub mod preferences;

use alloc::string::String;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Radio(crate::services::radio::Command),
    ScreenOff,
    Update(crate::services::update::Command),
    SelectSkin(String),
}
