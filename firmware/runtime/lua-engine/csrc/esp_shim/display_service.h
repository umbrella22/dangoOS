/* The display_service subset used by esp-claw's display module. The single
 * session is backed by the Rust host (mosaico_display_* in vm.rs). */
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "esp_err.h"
#include "sdkconfig.h"

typedef struct display_service_session_t *display_service_session_handle_t;

typedef enum {
    DISPLAY_SERVICE_PANEL_INTERFACE_OTHER = 0,
    DISPLAY_SERVICE_PANEL_INTERFACE_SPI,
    DISPLAY_SERVICE_PANEL_INTERFACE_QSPI,
    DISPLAY_SERVICE_PANEL_INTERFACE_RGB,
    DISPLAY_SERVICE_PANEL_INTERFACE_MIPI_DSI,
} display_service_panel_interface_t;

typedef struct {
    uint16_t width;
    uint16_t height;
    uint32_t bits_per_pixel;
    display_service_panel_interface_t panel_interface;
    bool touch_available;
} display_service_info_t;

typedef enum {
    DISPLAY_SERVICE_MODE_LVGL = 0,
    DISPLAY_SERVICE_MODE_EXCLUSIVE_RAW,
} display_service_mode_t;

typedef void (*display_service_session_cleanup_cb_t)(display_service_session_handle_t session, void *user_ctx);
typedef void (*display_service_session_exit_request_cb_t)(display_service_session_handle_t session, void *user_ctx);

typedef struct {
    uint32_t buffer_lines;
    uint32_t tick_ms;
    uint32_t task_period_ms;
} display_service_config_t;

typedef struct {
    const char *owner_name;
    display_service_mode_t mode;
    uint32_t flags;
    display_service_config_t display_config;
    display_service_session_cleanup_cb_t cleanup_cb;
    display_service_session_exit_request_cb_t exit_request_cb;
    void *user_ctx;
} display_service_session_config_t;

typedef struct {
    int x_start;
    int y_start;
    int x_end;
    int y_end;
    const void *frame_buffer;
    size_t stride_bytes;
    bool wait;
} display_service_raw_blit_t;

typedef struct {
    uint8_t id;
    int32_t x;
    int32_t y;
} display_service_touch_point_t;

typedef struct {
    uint8_t count;
    display_service_touch_point_t points[CONFIG_ESP_LCD_TOUCH_MAX_POINTS];
} display_service_touch_snapshot_t;

esp_err_t display_service_open(const display_service_session_config_t *config,
                               display_service_session_handle_t *ret_session);
esp_err_t display_service_close(display_service_session_handle_t session);
bool display_service_session_is_valid(display_service_session_handle_t session);
esp_err_t display_service_session_get_info(display_service_session_handle_t session, display_service_info_t *info);
esp_err_t display_service_session_raw_blit(display_service_session_handle_t session,
                                           const display_service_raw_blit_t *blit);
esp_err_t display_service_session_get_touch_snapshot(display_service_session_handle_t session,
                                                     display_service_touch_snapshot_t *snapshot);
