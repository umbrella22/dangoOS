#pragma once
#include <stddef.h>
#include "esp_err.h"
typedef unsigned nvs_handle_t;
#define NVS_NS_NAME_MAX_SIZE 16
#define NVS_READWRITE 1
#define NVS_READONLY 0
#define ESP_ERR_NVS_NOT_FOUND 0x1102
#define ESP_ERR_NVS_INVALID_LENGTH 0x110c
#define ESP_ERR_NVS_NOT_INITIALIZED 0x1101
#define nvs_open mosaico_bt_nvs_open
#define nvs_close mosaico_bt_nvs_close
#define nvs_get_blob mosaico_bt_nvs_get_blob
#define nvs_set_blob mosaico_bt_nvs_set_blob
#define nvs_erase_key mosaico_bt_nvs_erase_key
#define nvs_erase_all mosaico_bt_nvs_erase_all
#define nvs_commit mosaico_bt_nvs_commit
int mosaico_bt_nvs_open(const char*,int,nvs_handle_t*);
void mosaico_bt_nvs_close(nvs_handle_t);
int mosaico_bt_nvs_get_blob(nvs_handle_t,const char*,void*,size_t*);
int mosaico_bt_nvs_set_blob(nvs_handle_t,const char*,const void*,size_t);
int mosaico_bt_nvs_erase_key(nvs_handle_t,const char*);
int mosaico_bt_nvs_erase_all(nvs_handle_t);
int mosaico_bt_nvs_commit(nvs_handle_t);
