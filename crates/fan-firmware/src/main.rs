//! Bare-metal ESP32-S31 application entry point.
#![no_std]
#![no_main]

extern crate alloc;

mod app;
mod board;
mod drivers;
mod platform_mosaic;
mod quick_feedback;
mod runtime;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

#[unsafe(no_mangle)]
fn custom_halt() -> ! {
    esp_hal::delay::Delay::new().delay_millis(500);
    esp_hal::system::software_reset()
}

#[esp_hal::main]
fn main() -> ! {
    app::run()
}
