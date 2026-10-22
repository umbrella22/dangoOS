//! The firmware's Embassy tasks. Each one owns its hardware exclusively.
//!
//! - [`plane`]: uploads the newest fox canvas to the panel (interrupt executor).
//! - [`ui`]: owns LVGL and the behaviour runtime; renders the fox.
//! - [`input`]: reads the touch controller into a bounded queue.
//! - [`sensors`]: IMU, magnetometers, fuel gauge and the module slot.
//! - [`haptic`]: motor patterns.
//! - [`lights`]: Interaction board WS2812 mood light.
//! - [`lua`]: the Lua 5.5 runtime (L0: boot self-test).

pub mod haptic;
pub mod input;
pub mod lights;
pub mod lua;
pub mod plane;
pub mod sensors;
pub mod ui;
