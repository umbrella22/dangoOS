#pragma once
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include "esp_err.h"
#include "lua.h"
typedef enum { LUA_IMAGE_FORMAT_RGB565LE, LUA_IMAGE_FORMAT_RGB565BE, LUA_IMAGE_FORMAT_RGB888, LUA_IMAGE_FORMAT_BGR888, LUA_IMAGE_FORMAT_GRAY8, LUA_IMAGE_FORMAT_YUYV, LUA_IMAGE_FORMAT_UYVY, LUA_IMAGE_FORMAT_JPEG, LUA_IMAGE_FORMAT_MJPEG, LUA_IMAGE_FORMAT_COUNT } lua_image_format_t;
typedef struct { const uint8_t *data; size_t bytes; int width,height; lua_image_format_t format; bool owned; char source_format[8]; } lua_image_view_t;
esp_err_t lua_image_require_format(lua_State *L,int index,lua_image_format_t format,lua_image_view_t *view);
void lua_image_release_view(lua_image_view_t *view);
int luaopen_image(lua_State *L);
