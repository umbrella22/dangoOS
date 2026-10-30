//! The firmware's Embassy tasks. Each one owns its hardware exclusively.
//!
//! - [`plane`]: uploads the newest fox canvas to the panel (interrupt executor).
//! - [`ui`]: owns LVGL and the behaviour runtime; renders the fox.
//! - [`input`]: reads the touch controller into a bounded queue.
//! - [`sensors`]: IMU, magnetometers, fuel gauge and the module slot.
//! - [`haptic`]: motor patterns.
//! - [`lights`]: Interaction board WS2812 mood light.
//! - [`lua`]: the Lua 5.5 runtime that runs one app at a time.
//! - [`apps`]: the app registry scan and requests from the USB push protocol.

pub mod apps;
pub mod brain_control;
pub mod buttons;
pub mod battery;
pub mod camera;
pub mod haptic;
pub mod input;
pub mod lights;
pub mod lua;
pub mod plane;
pub mod power;
pub mod radio;
pub mod sensors;
pub mod settings;
pub mod skins;
pub mod ui;
pub mod update;

pub mod audio;
