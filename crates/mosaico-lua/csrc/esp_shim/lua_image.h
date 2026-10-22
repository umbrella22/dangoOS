/* Image objects are not available yet; blit_image reports NOT_SUPPORTED. */
#pragma once
#include <stddef.h>
#include <stdint.h>
#include "esp_err.h"
#include "lua.h"

typedef enum { LUA_IMAGE_FORMAT_RGB565LE, LUA_IMAGE_FORMAT_BGR888 } lua_image_format_t;
typedef struct { const uint8_t *data; size_t bytes; int width, height; } lua_image_view_t;

esp_err_t lua_image_require_format(lua_State *L, int index, lua_image_format_t format, lua_image_view_t *view);
void lua_image_release_view(lua_image_view_t *view);
