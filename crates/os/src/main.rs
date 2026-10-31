//! ESP-Mosaico firmware entry and stable OTA identity.
#![no_std]
#![no_main]

// Existing devices validate this OTA identity; Cargo package names may change.
esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),
    "mosaico-s31",
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
    os::boot::run(spawner).await;
}
