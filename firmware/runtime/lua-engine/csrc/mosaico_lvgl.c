/* Handles never contain native LVGL pointers. All mutations cross the UI owner. */
#include "lua.h"
#include "lauxlib.h"
#include <stdint.h>
extern const char mosaico_lvgl_lua[];
extern const size_t mosaico_lvgl_lua_len;
#define HANDLE "mosaico.lvgl.object"
static int handle_index(lua_State*L){luaL_checkudata(L,1,HANDLE);lua_getiuservalue(L,1,1);lua_pushvalue(L,2);lua_gettable(L,-2);return 1;}
static int make_handle(lua_State*L){lua_Integer id=luaL_checkinteger(L,1);luaL_checktype(L,2,LUA_TTABLE);lua_Integer*p=lua_newuserdatauv(L,sizeof(*p),1);*p=id;lua_pushvalue(L,2);lua_setiuservalue(L,-2,1);if(luaL_newmetatable(L,HANDLE)){lua_pushcfunction(L,handle_index);lua_setfield(L,-2,"__index");lua_pushboolean(L,0);lua_setfield(L,-2,"__metatable");}lua_setmetatable(L,-2);return 1;}
static int handle_id(lua_State*L){lua_Integer*p=luaL_checkudata(L,1,HANDLE);lua_pushinteger(L,*p);return 1;}
int luaopen_lvgl(lua_State*L){lua_getglobal(L,"_lvgl");lua_pushcfunction(L,make_handle);lua_setfield(L,-2,"handle");lua_pushcfunction(L,handle_id);lua_setfield(L,-2,"id");lua_pop(L,1);if(luaL_loadbufferx(L,mosaico_lvgl_lua,mosaico_lvgl_lua_len,"=lvgl","t")!=LUA_OK)return lua_error(L);lua_call(L,0,1);return 1;}

extern const char mosaico_thread_lua[];
extern const size_t mosaico_thread_lua_len;
int luaopen_thread(lua_State*L){if(luaL_loadbufferx(L,mosaico_thread_lua,mosaico_thread_lua_len,"=thread","t")!=LUA_OK)return lua_error(L);lua_call(L,0,1);return 1;}
