#pragma once
#include <stdint.h>
#include <stdbool.h>
#include "esp_err.h"
typedef void *esp_timer_handle_t;
typedef void (*esp_timer_cb_t)(void*);
typedef struct { esp_timer_cb_t callback;void*arg;int dispatch_method;const char*name;bool skip_unhandled_events; } esp_timer_create_args_t;
#define ESP_TIMER_TASK 0
#define esp_timer_create mosaico_bt_timer_create
#define esp_timer_start_once mosaico_bt_timer_once
#define esp_timer_start_periodic mosaico_bt_timer_periodic
#define esp_timer_stop mosaico_bt_timer_stop
#define esp_timer_delete mosaico_bt_timer_delete
#define esp_timer_get_time mosaico_bt_time_us
#define esp_timer_is_active mosaico_bt_timer_active
esp_err_t mosaico_bt_timer_create(const esp_timer_create_args_t*,esp_timer_handle_t*);
esp_err_t mosaico_bt_timer_once(esp_timer_handle_t,uint64_t);
esp_err_t mosaico_bt_timer_periodic(esp_timer_handle_t,uint64_t);
esp_err_t mosaico_bt_timer_stop(esp_timer_handle_t);
esp_err_t mosaico_bt_timer_delete(esp_timer_handle_t);
int64_t mosaico_bt_time_us(void);
bool mosaico_bt_timer_active(esp_timer_handle_t);
