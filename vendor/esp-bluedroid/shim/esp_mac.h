#pragma once
#include <stdint.h>
#define ESP_MAC_BT 2
int mosaico_bt_read_mac(uint8_t*,int);
#define esp_read_mac mosaico_bt_read_mac
