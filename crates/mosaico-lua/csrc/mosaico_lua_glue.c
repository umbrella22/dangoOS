/*
 * C side of the Rust <-> Lua bridge.
 *
 * Rule: a Lua error (longjmp) must never cross a Rust frame. Every function
 * here that may raise runs either inside Lua's own protected calls or is a
 * lua_CFunction whose only Rust callee has already returned. Rust reports
 * failures with status codes and the error is raised from C.
 */
#include <stdio.h>
#include <string.h>

#include "lua.h"
#include "lauxlib.h"
#include "lualib.h"

/* ---- Implemented in Rust (crates/mosaico-lua/src/vm.rs) ---------------- */

/* 0 continue, 1 yield the script, 2 interrupted, 3 script never yields. */
int mosaico_lua_on_hook(void *ctx, lua_State *L, int yieldable);
void mosaico_lua_on_print(void *ctx, const char *s, size_t len);
/* >= 0 results, -1 error, -2 yield, -3 argument error. */
int mosaico_lua_call_rust(void *ctx, lua_State *L, void *fn);
const char *mosaico_lua_error_text(void *ctx, size_t *len, int *arg);
void mosaico_lua_log(const char *s, size_t len);

#define CALL_ERROR (-1)
#define CALL_YIELD (-2)
#define CALL_ARG_ERROR (-3)

static void *ctx_of(lua_State *L) { return *(void **)lua_getextraspace(L); }

void mosaico_lua_set_ctx(lua_State *L, void *ctx) {
  *(void **)lua_getextraspace(L) = ctx;
}

/* ---- Log sinks used by mosaico_lua_user.h ------------------------------ */

void mosaico_lua_raw_write(const char *s, size_t len) { mosaico_lua_log(s, len); }

void mosaico_lua_raw_errorf(const char *fmt, const char *arg) {
  char buf[160];
  int n = snprintf(buf, sizeof buf, fmt, arg);
  if (n < 0) return;
  if ((size_t)n >= sizeof buf) n = (int)sizeof buf - 1;
  mosaico_lua_log(buf, (size_t)n);
}

/* ---- print / warn ------------------------------------------------------ */

/* Same output as luaB_print, delivered as one line to the host. */
static int glue_print(lua_State *L) {
  int n = lua_gettop(L);
  luaL_Buffer b;
  luaL_buffinit(L, &b);
  for (int i = 1; i <= n; i++) {
    size_t l;
    const char *s = luaL_tolstring(L, i, &l); /* pushes the string */
    if (i > 1) luaL_addchar(&b, '\t');
    luaL_addvalue(&b); /* pops it */
    (void)s;
    (void)l;
  }
  luaL_pushresult(&b);
  size_t len;
  const char *line = lua_tolstring(L, -1, &len);
  mosaico_lua_on_print(ctx_of(L), line, len);
  return 0;
}

/* lauxlib's warn protocol: off by default, "@on"/"@off" control messages. */
static int warn_on = 0;
static void glue_warn(void *ud, const char *msg, int tocont) {
  static int continuing = 0;
  (void)ud;
  if (!continuing && !tocont && msg[0] == '@') {
    if (strcmp(msg + 1, "on") == 0) warn_on = 1;
    else if (strcmp(msg + 1, "off") == 0) warn_on = 0;
    return;
  }
  if (warn_on) {
    if (!continuing) mosaico_lua_log("Lua warning: ", 13);
    mosaico_lua_log(msg, strlen(msg));
  }
  continuing = tocont;
}

static int glue_panic(lua_State *L) {
  const char *msg = lua_tostring(L, -1);
  mosaico_lua_raw_errorf("Lua panic: %s", msg ? msg : "(error object is not a string)");
  return 0; /* Lua aborts after this */
}

/* ---- State setup ------------------------------------------------------- */

static const luaL_Reg libs[] = {
    {LUA_GNAME, luaopen_base},          {LUA_LOADLIBNAME, luaopen_package},
    {LUA_COLIBNAME, luaopen_coroutine}, {LUA_DBLIBNAME, luaopen_debug},
    {LUA_MATHLIBNAME, luaopen_math},    {LUA_STRLIBNAME, luaopen_string},
    {LUA_TABLIBNAME, luaopen_table},    {LUA_UTF8LIBNAME, luaopen_utf8},
    {NULL, NULL}};

static int open_impl(lua_State *L) {
  for (const luaL_Reg *lib = libs; lib->func; lib++) {
    luaL_requiref(L, lib->name, lib->func, 1);
    lua_pop(L, 1);
  }
  lua_pushcfunction(L, glue_print);
  lua_setglobal(L, "print");
  /* No file system yet: drop the searchers that would reach fopen/dlopen,
     keeping only package.preload. Apps get their own searcher in L2. */
  lua_getglobal(L, LUA_LOADLIBNAME);
  lua_getfield(L, -1, "searchers");
  for (lua_Integer i = (lua_Integer)luaL_len(L, -1); i > 1; i--) {
    lua_pushnil(L);
    lua_rawseti(L, -2, i);
  }
  lua_pop(L, 2);
  /* dofile/loadfile would read stdin or a file. */
  lua_pushnil(L);
  lua_setglobal(L, "dofile");
  lua_pushnil(L);
  lua_setglobal(L, "loadfile");
  return 0;
}

int mosaico_lua_open(lua_State *L) {
  lua_atpanic(L, glue_panic);
  lua_setwarnf(L, glue_warn, NULL);
  lua_pushcfunction(L, open_impl);
  return lua_pcall(L, 0, 0, 0);
}

/* ---- Rust functions ---------------------------------------------------- */

static int trampoline(lua_State *L) {
  void *ctx = ctx_of(L);
  int n = mosaico_lua_call_rust(ctx, L, lua_touserdata(L, lua_upvalueindex(1)));
  if (n >= 0) return n;
  if (n == CALL_YIELD) return lua_yield(L, 0);
  size_t len;
  int arg;
  const char *text = mosaico_lua_error_text(ctx, &len, &arg);
  if (n == CALL_ARG_ERROR) {
    lua_pushlstring(L, text, len);
    return luaL_argerror(L, arg, lua_tostring(L, -1));
  }
  luaL_where(L, 1);
  lua_pushlstring(L, text, len);
  lua_concat(L, 2);
  return lua_error(L);
}

typedef struct {
  const char *name;
  void *fn;
} mosaico_lua_reg;

typedef struct {
  const char *module;
  const mosaico_lua_reg *regs;
  size_t count;
} register_args;

static int register_impl(lua_State *L) {
  const register_args *a = (const register_args *)lua_touserdata(L, 1);
  /* Reuse an existing global table (e.g. extend `os`), else make one. */
  if (lua_getglobal(L, a->module) != LUA_TTABLE) {
    lua_pop(L, 1);
    lua_createtable(L, 0, (int)a->count);
  }
  for (size_t i = 0; i < a->count; i++) {
    lua_pushlightuserdata(L, a->regs[i].fn);
    lua_pushcclosure(L, trampoline, 1);
    lua_setfield(L, -2, a->regs[i].name);
  }
  luaL_getsubtable(L, LUA_REGISTRYINDEX, LUA_LOADED_TABLE);
  lua_pushvalue(L, -2);
  lua_setfield(L, -2, a->module); /* package.loaded[module] = t */
  lua_pop(L, 1);
  lua_setglobal(L, a->module); /* like luaL_requiref(..., 1) */
  return 0;
}

int mosaico_lua_register(lua_State *L, const char *module, const mosaico_lua_reg *regs,
                         size_t count) {
  register_args a = {module, regs, count};
  lua_pushcfunction(L, register_impl);
  lua_pushlightuserdata(L, &a);
  return lua_pcall(L, 1, 0, 0);
}

/* ---- Scheduling hook --------------------------------------------------- */

static void glue_hook(lua_State *L, lua_Debug *ar) {
  (void)ar;
  switch (mosaico_lua_on_hook(ctx_of(L), L, lua_isyieldable(L))) {
  case 1:
    lua_yield(L, 0);
    return;
  case 2:
    luaL_error(L, "interrupted");
    return;
  case 3:
    luaL_error(L, "script ran too long without yielding");
    return;
  default:
    return;
  }
}

void mosaico_lua_install_hook(lua_State *L, int count) {
  lua_sethook(L, glue_hook, LUA_MASKCOUNT, count);
}

/* ---- Error reporting --------------------------------------------------- */

static int traceback_impl(lua_State *L) {
  lua_State *thread = (lua_State *)lua_touserdata(L, 1);
  const char *msg = lua_tostring(thread, -1);
  if (msg == NULL) msg = "(error object is not a string)";
  luaL_traceback(L, thread, msg, 0);
  return 1;
}

/* Pushes "message\nstack traceback:..." for a failed thread onto L. */
int mosaico_lua_traceback(lua_State *L, lua_State *thread) {
  lua_pushcfunction(L, traceback_impl);
  lua_pushlightuserdata(L, thread);
  return lua_pcall(L, 1, 1, 0);
}

/* ---- Script thread ----------------------------------------------------- */

static int thread_impl(lua_State *L) {
  lua_State **out = (lua_State **)lua_touserdata(L, 1);
  *out = lua_newthread(L);
  lua_setfield(L, LUA_REGISTRYINDEX, "mosaico.script"); /* anchor it */
  return 0;
}

/* The coroutine that runs the script, or NULL (message pushed on L). */
lua_State *mosaico_lua_new_thread(lua_State *L) {
  lua_State *thread = NULL;
  lua_pushcfunction(L, thread_impl);
  lua_pushlightuserdata(L, &thread);
  if (lua_pcall(L, 1, 0, 0) != LUA_OK) return NULL;
  return thread;
}
