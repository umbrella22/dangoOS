#![no_std]
#![no_main]

esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),
    "dango-recovery",
    esp_bootloader_esp_idf::BUILD_TIME,
    esp_bootloader_esp_idf::BUILD_DATE,
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    recovery::device::run(spawner).await;
}

// Use esp-hal's stack failure handler without pulling newlib's second copy.
#[no_mangle]
pub static mut __stack_chk_guard: u32 = 0xe2de_e396;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    esp_println::println!("recovery panic: {}", info);
    // A stalled executor cannot service RCR1. Leave a usable ROM downloader
    // instead of retaining an enumerated but unresponsive rescue interface.
    if board_s31::reset::download_available() {
        board_s31::reset::to_rom();
    }
    esp_hal::system::software_reset()
}
