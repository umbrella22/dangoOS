idf_component_register(SRCS "bootloader_start.c" "display.c"
                       REQUIRES bootloader bootloader_support esp_hal_gpspi esp_hal_gpio)
idf_build_get_property(scripts BOOTLOADER_LINKER_SCRIPT)
target_linker_script(${COMPONENT_LIB} INTERFACE "${scripts}")
