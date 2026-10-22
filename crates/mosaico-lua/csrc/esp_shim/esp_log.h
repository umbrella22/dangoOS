/* Log macros of the vendored modules go to the Lua log sink. */
#pragma once
#include "esp_err.h"

void mosaico_lua_logf(char level, const char *tag, const char *fmt, ...) __attribute__((format(printf, 3, 4)));

#define ESP_LOGE(tag, fmt, ...) mosaico_lua_logf('E', tag, fmt, ##__VA_ARGS__)
#define ESP_LOGW(tag, fmt, ...) mosaico_lua_logf('W', tag, fmt, ##__VA_ARGS__)
#define ESP_LOGI(tag, fmt, ...) mosaico_lua_logf('I', tag, fmt, ##__VA_ARGS__)
#define ESP_LOGD(tag, fmt, ...) ((void)(tag))
#define ESP_LOGV(tag, fmt, ...) ((void)(tag))
