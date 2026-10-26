#ifndef MOSAICO_HARDWARE_H
#define MOSAICO_HARDWARE_H
#include "lua.h"
int luaopen_mosaico_hardware(lua_State *L);
void mosaico_hardware_preload(lua_State *L);
#endif
