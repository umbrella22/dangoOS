//! Explicit ROM-download entry. Only a volatile LP-system register is changed;
//! no eFuse or persistent storage operation is performed.
pub fn download_available() -> bool {
    esp_hal::efuse::read_field_le::<u8>(esp_hal::efuse::DIS_FORCE_DOWNLOAD) == 0
}

/// The caller has finished storage operations and acknowledged the USB command.
pub fn to_rom() -> ! {
    esp_hal::peripherals::LP_SYS::regs()
        .sys_ctrl()
        .modify(|_, w| w.force_download_boot().set_bit());
    esp_hal::system::software_reset()
}
