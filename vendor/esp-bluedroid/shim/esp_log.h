#pragma once
#include <stdint.h>
#include <stdio.h>
#include <inttypes.h>
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#define ESP_LOG_NONE 0
#define ESP_LOG_ERROR 1
#define ESP_LOG_WARN 2
#define ESP_LOG_INFO 3
#define ESP_LOG_DEBUG 4
#define ESP_LOG_VERBOSE 5
#define LOG_LOCAL_LEVEL 2
typedef int esp_log_level_t;
void mosaico_bt_log(int,const char*,const char*,...);
#define ESP_LOGE(tag,fmt,...) mosaico_bt_log(1,tag,fmt,##__VA_ARGS__)
#define ESP_LOGW(tag,fmt,...) mosaico_bt_log(2,tag,fmt,##__VA_ARGS__)
#define ESP_LOGI(tag,fmt,...) mosaico_bt_log(3,tag,fmt,##__VA_ARGS__)
#define ESP_LOGD(tag,fmt,...) do {} while(0)
#define ESP_LOGV(tag,fmt,...) do {} while(0)
#define ESP_LOG_LEVEL_LOCAL(level,tag,fmt,...) mosaico_bt_log(level,tag,fmt,##__VA_ARGS__)
#define ESP_LOG_LEVEL(level,tag,fmt,...) mosaico_bt_log(level,tag,fmt,##__VA_ARGS__)
#define ESP_LOG_BUFFER_HEX_LEVEL(tag,buf,len,level) do {} while(0)
#define esp_log_timestamp mosaico_bt_ticks
#define esp_log_level_get(tag) ESP_LOG_WARN
#define esp_log_level_set(tag,level) do {} while(0)

#define esp_log_write mosaico_bt_log
#define LOG_FORMAT(letter,fmt) "%lu %s: " fmt "\n"
