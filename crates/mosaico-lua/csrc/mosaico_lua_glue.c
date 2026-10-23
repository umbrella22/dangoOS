#include <setjmp.h>
#include "lstate.h"
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
#include "mosaico_platform.h"

/* ---- Implemented in Rust (crates/mosaico-lua/src/vm.rs) ---------------- */

/* 0 continue, 1 yield the script, 2 interrupted, 3 script never yields. */
int mosaico_lua_on_hook(void *ctx, lua_State *L, int yieldable);
void mosaico_lua_on_print(void *ctx, const char *s, size_t len);
/* >= 0 results, -1 error, -2 yield, -3 argument error. */
int mosaico_lua_call_rust(void *ctx, lua_State *L, void *fn);
const char *mosaico_lua_error_text(void *ctx, size_t *len, int *arg);
void mosaico_lua_log(const char *s, size_t len);
/* 1 when the request was recorded and the caller must yield now. */
int mosaico_lua_on_request(void *ctx, lua_State *L, int yieldable, int kind, unsigned arg);
int mosaico_lua_on_stop(void *ctx);

#define CALL_ERROR (-1)
#define CALL_YIELD (-2)
#define CALL_ARG_ERROR (-3)

static void *ctx_of(lua_State *L) { return *(void **)lua_getextraspace(L); }

/* Used by mosaico_platform.c (delay, display:present). */
int mosaico_lua_request(lua_State *L, int kind, unsigned arg) {
  return mosaico_lua_on_request(ctx_of(L), L, lua_isyieldable(L), kind, arg);
}
int mosaico_lua_stop_requested(lua_State *L) { return mosaico_lua_on_stop(ctx_of(L)); }

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
    {LUA_OSLIBNAME, luaopen_os},        {LUA_IOLIBNAME, luaopen_io},
    {NULL, NULL}};

/* Same as esp-claw cap_lua_restrict_debug: scripts get traceback and
   getinfo only (sethook etc. would break the scheduler). */
static void restrict_debug(lua_State *L) {
  lua_getglobal(L, "debug");
  lua_newtable(L);
  lua_getfield(L, -2, "traceback");
  lua_setfield(L, -2, "traceback");
  lua_getfield(L, -2, "getinfo");
  lua_setfield(L, -2, "getinfo");
  lua_pushvalue(L, -1);
  lua_setglobal(L, "debug");
  lua_getfield(L, LUA_REGISTRYINDEX, LUA_LOADED_TABLE);
  lua_pushvalue(L, -2);
  lua_setfield(L, -2, "debug");
  lua_pop(L, 3);
}

static int open_impl(lua_State *L) {
  for (const luaL_Reg *lib = libs; lib->func; lib++) {
    luaL_requiref(L, lib->name, lib->func, 1);
    lua_pop(L, 1);
  }
  lua_pushcfunction(L, glue_print);
  lua_setglobal(L, "print");
  /* Keep the preload and Lua-file searchers; drop the C-library ones
     (there is no dlopen). Files come from the VFS (/system, /data). */
  lua_getglobal(L, LUA_LOADLIBNAME);
  lua_getfield(L, -1, "searchers");
  for (lua_Integer i = (lua_Integer)luaL_len(L, -1); i > 2; i--) {
    lua_pushnil(L);
    lua_rawseti(L, -2, i);
  }
  lua_pop(L, 2);
  /* os.exit would end the firmware. */
  lua_getglobal(L, LUA_OSLIBNAME);
  lua_pushnil(L);
  lua_setfield(L, -2, "exit");
  lua_pop(L, 1);
  restrict_debug(L);
  mosaico_platform_preload(L);
  /* esp-claw always defines `args` (a table; filled by Vm::set_args). */
  lua_newtable(L);
  lua_setglobal(L, "args");
  return 0;
}

/* package.path = prefix .. ";" .. package.path (esp-claw
   cap_lua_add_script_dir_to_package_path). */
static int prepend_path_impl(lua_State *L) {
  const char *prefix = (const char *)lua_touserdata(L, 1);
  lua_getglobal(L, LUA_LOADLIBNAME);
  lua_getfield(L, -1, "path");
  const char *cur = lua_tostring(L, -1);
  if (cur && cur[0])
    lua_pushfstring(L, "%s;%s", prefix, cur);
  else
    lua_pushstring(L, prefix);
  lua_setfield(L, -3, "path");
  return 0;
}

int mosaico_lua_prepend_path(lua_State *L, const char *prefix) {
  lua_pushcfunction(L, prepend_path_impl);
  lua_pushlightuserdata(L, (void *)prefix);
  return lua_pcall(L, 1, 0, 0);
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

/* A prepared text chunk is bound to its restricted environment only when
 * the trusted dispatcher starts. No parser or user execution runs here. */
static int bind_prepared_chunk(lua_State *L) {
  luaL_checktype(L, 1, LUA_TTABLE);
  lua_pushvalue(L, lua_upvalueindex(1));
  lua_pushvalue(L, 1);
  const char *name = lua_setupvalue(L, -2, 1);
  if (!name || strcmp(name, "_ENV") != 0)
    return luaL_error(L, "prepared chunk has no environment");
  return 1;
}

typedef struct {
  const char *module, *name, *source;
  size_t len;
} prepare_chunk_args;

static int prepare_chunk_impl(lua_State *L) {
  const prepare_chunk_args *a = lua_touserdata(L, 1);
  if (luaL_loadbufferx(L, a->source, a->len, a->name, "t") != LUA_OK)
    return lua_error(L);
  lua_pushcclosure(L, bind_prepared_chunk, 1);
  luaL_getsubtable(L, LUA_REGISTRYINDEX, LUA_LOADED_TABLE);
  lua_pushvalue(L, -2);
  lua_setfield(L, -2, a->module);
  return 0;
}

int mosaico_lua_prepare_chunk(lua_State *L, const char *module, const char *name,
                              const char *source, size_t len) {
  prepare_chunk_args a = {module, name, source, len};
  lua_pushcfunction(L, prepare_chunk_impl);
  lua_pushlightuserdata(L, &a);
  return lua_pcall(L, 1, 0, 0);
}

/* ---- Scheduling hook --------------------------------------------------- */

extern void *mosaico_lua_swap_abort_guard(void *ctx, void *guard);
extern void *mosaico_lua_abort_guard(void *ctx);

/* The setjmp boundary lives entirely in C, outside lua_resume. A hard budget
 * escape cannot be caught by Lua pcall and never jumps across a Rust frame. */
int mosaico_lua_resume_bounded(lua_State *L, int *nres) {
  jmp_buf guard;
  void *previous = mosaico_lua_swap_abort_guard(ctx_of(L), &guard);
  int status;
  if (setjmp(guard) == 0) status = lua_resume(L, NULL, 0, nres);
  else status = -2;
  mosaico_lua_swap_abort_guard(ctx_of(L), previous);
  return status;
}
void mosaico_lua_restrict_finalizers(lua_State *L) {
  G(L)->mosaico_skip_finalizers = 1;
}
void mosaico_lua_discard(lua_State *L) {
  G(L)->mosaico_skip_finalizers = 1;
  lua_close(L);
}

static void glue_hook(lua_State *L, lua_Debug *ar) {
  (void)ar;
  switch (mosaico_lua_on_hook(ctx_of(L), L, lua_isyieldable(L))) {
  case 1:
    lua_yield(L, 0);
    return;
  case 2:
    luaL_error(L, "interrupted");
    return;
  case 4: {
    void *guard = mosaico_lua_abort_guard(ctx_of(L));
    if (guard) longjmp(*(jmp_buf *)guard, 1);
    luaL_error(L, "callback budget exhausted");
    return;
  }
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

/* ---- args and exit ------------------------------------------------------ */

static int set_args_impl(lua_State *L) {
  /* args = require("json").decode(text) when that yields a table */
  lua_getglobal(L, "require");
  lua_pushliteral(L, "json");
  lua_call(L, 1, 1);
  lua_getfield(L, -1, "decode");
  lua_pushvalue(L, 1);
  if (lua_pcall(L, 1, 1, 0) == LUA_OK && lua_istable(L, -1)) {
    lua_setglobal(L, "args");
  }
  return 0;
}

int mosaico_lua_set_args(lua_State *L, const char *json, size_t len) {
  lua_pushcfunction(L, set_args_impl);
  lua_pushlstring(L, json, len);
  return lua_pcall(L, 1, 0, 0);
}

static int exit_impl(lua_State *L) {
  mosaico_platform_exit_cleanups(L);
  return 0;
}

void mosaico_lua_exit(lua_State *L) {
  lua_pushcfunction(L, exit_impl);
  if (lua_pcall(L, 0, 0, 0) != LUA_OK) lua_pop(L, 1);
}

/* Workers have no native resource handles. This is called before their first
 * instruction; bounded close may then skip arbitrary script finalizers safely. */
int mosaico_lua_worker_restrict(lua_State *L) {
  static const char source[] =
    "local original=require; local allowed={_G=true,coroutine=true,table=true,string=true,math=true,utf8=true,json=true,delay=true,thread=true,_thread=true};"
    "require=function(name) if not allowed[name] then error('module unavailable in worker: '..tostring(name),2) end return original(name) end;"
    "package=nil;debug=nil;io=nil;os=nil;loadfile=nil;dofile=nil;_lvgl=nil;";
  if(luaL_loadbufferx(L,source,sizeof(source)-1,"=worker-capabilities","t")!=LUA_OK)return LUA_ERRRUN;
  return lua_pcall(L,0,0,0);
}
