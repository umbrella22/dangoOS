//! LVGL 9.6 official widgets demo - ESP32-S31 (ESP-Mosaico v1.2) firmware.
#![no_std]
#![no_main]

extern crate alloc;

mod board;
mod drivers;
mod lvgl_ui;

use esp_backtrace as _;

esp_bootloader_esp_idf::esp_app_desc!();

#[unsafe(no_mangle)]
fn custom_halt() -> ! {
    esp_hal::delay::Delay::new().delay_millis(500);
    esp_hal::system::software_reset()
}

#[esp_hal::main]
fn main() -> ! {
    lvgl_ui::run()
}
