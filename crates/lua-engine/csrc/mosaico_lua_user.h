/*
 * Force-included before every Lua source file (gcc -include).
 *
 * The firmware has no locale, no stdout and no wall clock, so the few
 * luaconf.h / lauxlib.h hooks that would reach them are redirected here.
 * Nothing in this header changes Lua semantics for scripts.
 */
#ifndef MOSAICO_LUA_USER_H
#define MOSAICO_LUA_USER_H

#include <stddef.h>
#include <string.h>

/* No locales: the decimal point is always '.', strings compare bytewise. */
#define lua_getlocaledecpoint() ('.')
#define l_strcoll(a, b) strcmp((a), (b))

/* The seed is passed to lua_newstate by Rust; luaL_newstate is unused. */
#define luai_makeseed() (0u)

/*
 * lauxlib/lbaselib/ldblib write through these. `print` is replaced by the
 * glue (it needs the state to find its host), so these only carry Lua's own
 * warnings and panic messages to the firmware log.
 */
void mosaico_lua_raw_write(const char *s, size_t len);
void mosaico_lua_raw_errorf(const char *fmt, const char *arg);
#define lua_writestring(s, l) mosaico_lua_raw_write((s), (l))
#define lua_writeline() mosaico_lua_raw_write("\n", 1)
#define lua_writestringerror(s, p) mosaico_lua_raw_errorf((s), (p))

#endif
