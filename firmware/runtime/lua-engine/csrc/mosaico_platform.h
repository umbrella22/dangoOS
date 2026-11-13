/* Hooks between the vendored esp-claw modules and the Mosaico runtime. */
#pragma once
#include <stdbool.h>
#include "lua.h"

/* Called by display:present() after the frame was handed to the host. Yields
 * the script (Request::Present) when it can, so the host shows the frame
 * before the script draws the next one. Returns the Lua result count. */
int mosaico_display_presented(lua_State *L, bool updated);

/* Opens the esp-claw modules as package.preload entries. */
void mosaico_platform_preload(lua_State *L);
/* Runs cap_lua exit cleanups (display close) before lua_close. */
void mosaico_platform_exit_cleanups(lua_State *L);
