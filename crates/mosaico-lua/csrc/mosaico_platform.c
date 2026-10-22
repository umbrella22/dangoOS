/*
 * Platform layer for the vendored esp-claw Lua modules (display, json,
 * arg_schema) plus the delay and system modules, which are written here
 * against the official READMEs because the originals depend on FreeRTOS and
 * ESP-IDF services.
 *
 * Everything that touches hardware or time goes through mosaico_host_*,
 * implemented in Rust (src/platform.rs).
 */
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

#include "lauxlib.h"
#include "lua.h"
#include "lualib.h"

#include "cap_lua.h"
#include "display_service.h"
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "lua_image.h"
#include "lua_module_display.h"
#include "lua_module_json.h"
#include "lua_module_storage.h"
#include "mosaico_platform.h"

/* ---- Rust side ------------------------------------------------------- */

typedef struct {
    uint32_t total_size, free_size, allocated_size, minimum_free_size, largest_free_block;
    uint32_t allocated_blocks, free_blocks, total_blocks;
} mosaico_heap_info_t;

extern void *mosaico_host_alloc(size_t size, size_t align);
extern void mosaico_host_free(void *ptr);
extern int64_t mosaico_host_now_us(void);
extern int64_t mosaico_host_unix_us(void);
extern void mosaico_host_log(char level, const char *tag, const char *msg);
extern int mosaico_host_display_open(display_service_info_t *info);
extern int mosaico_host_display_info(display_service_info_t *info);
extern int mosaico_host_display_close(void);
extern int mosaico_host_display_blit(int x0, int y0, int x1, int y1, const void *fb, size_t stride);
extern int mosaico_host_display_touch(display_service_touch_snapshot_t *snapshot);
extern void mosaico_host_heap_info(uint32_t caps, mosaico_heap_info_t *out);
/* Asks the host to suspend the script. kind: 1 sleep(arg ms), 2 present.
 * Returns 1 when the caller must lua_yieldk now, 0 when it cannot yield. */
extern int mosaico_lua_request(lua_State *L, int kind, uint32_t arg);
extern int mosaico_lua_stop_requested(lua_State *L);

/* ---- ESP-IDF shims --------------------------------------------------- */

const char *esp_err_to_name(esp_err_t code)
{
    switch (code) {
    case ESP_OK: return "ESP_OK";
    case ESP_FAIL: return "ESP_FAIL";
    case ESP_ERR_NO_MEM: return "ESP_ERR_NO_MEM";
    case ESP_ERR_INVALID_ARG: return "ESP_ERR_INVALID_ARG";
    case ESP_ERR_INVALID_STATE: return "ESP_ERR_INVALID_STATE";
    case ESP_ERR_INVALID_SIZE: return "ESP_ERR_INVALID_SIZE";
    case ESP_ERR_NOT_FOUND: return "ESP_ERR_NOT_FOUND";
    case ESP_ERR_NOT_SUPPORTED: return "ESP_ERR_NOT_SUPPORTED";
    case ESP_ERR_TIMEOUT: return "ESP_ERR_TIMEOUT";
    case ESP_ERR_INVALID_RESPONSE: return "ESP_ERR_INVALID_RESPONSE";
    case ESP_ERR_INVALID_CRC: return "ESP_ERR_INVALID_CRC";
    case ESP_ERR_INVALID_VERSION: return "ESP_ERR_INVALID_VERSION";
    case ESP_ERR_INVALID_MAC: return "ESP_ERR_INVALID_MAC";
    case ESP_ERR_NOT_FINISHED: return "ESP_ERR_NOT_FINISHED";
    case ESP_ERR_NOT_ALLOWED: return "ESP_ERR_NOT_ALLOWED";
    default: return "UNKNOWN ERROR";
    }
}

void mosaico_lua_logf(char level, const char *tag, const char *fmt, ...)
{
    char buf[192];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    mosaico_host_log(level, tag, buf);
}

void *heap_caps_aligned_alloc(size_t alignment, size_t size, uint32_t caps)
{
    (void)caps;
    return mosaico_host_alloc(size, alignment < 16 ? 16 : alignment);
}

void *heap_caps_malloc(size_t size, uint32_t caps)
{
    (void)caps;
    return mosaico_host_alloc(size, 16);
}

void *heap_caps_calloc(size_t n, size_t size, uint32_t caps)
{
    (void)caps;
    if (size && n > SIZE_MAX / size) return NULL;
    void *p = mosaico_host_alloc(n * size, 16);
    if (p) memset(p, 0, n * size);
    return p;
}

void heap_caps_free(void *ptr)
{
    if (ptr) mosaico_host_free(ptr);
}

int64_t esp_timer_get_time(void)
{
    return mosaico_host_now_us();
}

/* ---- display_service: one session at a time -------------------------- */

struct display_service_session_t {
    bool open;
};
static struct display_service_session_t s_session;

esp_err_t display_service_open(const display_service_session_config_t *config,
                               display_service_session_handle_t *ret_session)
{
    if (!config || !ret_session) return ESP_ERR_INVALID_ARG;
    *ret_session = NULL;
    if (config->mode != DISPLAY_SERVICE_MODE_EXCLUSIVE_RAW) return ESP_ERR_NOT_SUPPORTED;
    if (s_session.open) return ESP_ERR_INVALID_STATE;
    display_service_info_t info;
    esp_err_t err = mosaico_host_display_open(&info);
    if (err != ESP_OK) return err;
    s_session.open = true;
    *ret_session = &s_session;
    return ESP_OK;
}

esp_err_t display_service_close(display_service_session_handle_t session)
{
    if (session != &s_session || !s_session.open) return ESP_ERR_INVALID_STATE;
    s_session.open = false;
    return mosaico_host_display_close();
}

bool display_service_session_is_valid(display_service_session_handle_t session)
{
    return session == &s_session && s_session.open;
}

esp_err_t display_service_session_get_info(display_service_session_handle_t session, display_service_info_t *info)
{
    if (!info) return ESP_ERR_INVALID_ARG;
    if (!display_service_session_is_valid(session)) return ESP_ERR_INVALID_STATE;
    return mosaico_host_display_info(info);
}

esp_err_t display_service_session_raw_blit(display_service_session_handle_t session,
                                           const display_service_raw_blit_t *blit)
{
    if (!blit || !blit->frame_buffer) return ESP_ERR_INVALID_ARG;
    if (!display_service_session_is_valid(session)) return ESP_ERR_INVALID_STATE;
    if (blit->x_end <= blit->x_start || blit->y_end <= blit->y_start) return ESP_ERR_INVALID_ARG;
    return mosaico_host_display_blit(blit->x_start, blit->y_start, blit->x_end, blit->y_end,
                                     blit->frame_buffer, blit->stride_bytes);
}

esp_err_t display_service_session_get_touch_snapshot(display_service_session_handle_t session,
                                                     display_service_touch_snapshot_t *snapshot)
{
    if (!snapshot) return ESP_ERR_INVALID_ARG;
    if (!display_service_session_is_valid(session)) return ESP_ERR_INVALID_STATE;
    memset(snapshot, 0, sizeof(*snapshot));
    return mosaico_host_display_touch(snapshot);
}

/* ---- image (not available yet) ---------------------------------------- */

esp_err_t lua_image_require_format(lua_State *L, int index, lua_image_format_t format, lua_image_view_t *view)
{
    (void)L; (void)index; (void)format;
    if (view) memset(view, 0, sizeof(*view));
    return ESP_ERR_NOT_SUPPORTED;
}

void lua_image_release_view(lua_image_view_t *view)
{
    (void)view;
}

/* ---- cap_lua ---------------------------------------------------------- */

#define MAX_MODULES 16
#define MAX_CLEANUPS 8
static struct { const char *name; lua_CFunction open; } s_modules[MAX_MODULES];
static size_t s_module_count;
static cap_lua_exit_cleanup_fn_t s_cleanups[MAX_CLEANUPS];
static size_t s_cleanup_count;

esp_err_t cap_lua_register_module(const char *name, lua_CFunction open_fn)
{
    for (size_t i = 0; i < s_module_count; ++i)
        if (strcmp(s_modules[i].name, name) == 0) { s_modules[i].open = open_fn; return ESP_OK; }
    if (s_module_count == MAX_MODULES) return ESP_ERR_NO_MEM;
    s_modules[s_module_count].name = name;
    s_modules[s_module_count].open = open_fn;
    s_module_count++;
    return ESP_OK;
}

esp_err_t cap_lua_register_exit_cleanup(cap_lua_exit_cleanup_fn_t fn)
{
    for (size_t i = 0; i < s_cleanup_count; ++i)
        if (s_cleanups[i] == fn) return ESP_OK;
    if (s_cleanup_count == MAX_CLEANUPS) return ESP_ERR_NO_MEM;
    s_cleanups[s_cleanup_count++] = fn;
    return ESP_OK;
}

const char *cap_lua_runtime_job_id(lua_State *L)
{
    (void)L;
    return "mosaico";
}

esp_err_t cap_lua_stop_job(const char *job_id, uint32_t wait_ms, char *output, size_t output_size)
{
    (void)job_id; (void)wait_ms;
    if (output && output_size) output[0] = '\0';
    return ESP_ERR_NOT_SUPPORTED;
}

void mosaico_platform_exit_cleanups(lua_State *L)
{
    for (size_t i = 0; i < s_cleanup_count; ++i) s_cleanups[i](L);
}

/* ---- display present ---------------------------------------------------- */

static int present_k(lua_State *L, int status, lua_KContext ctx)
{
    (void)status; (void)ctx;
    lua_pushboolean(L, 1);
    return 1;
}

int mosaico_display_presented(lua_State *L, bool updated)
{
    if (updated && mosaico_lua_request(L, 2, 0)) {
        lua_settop(L, 0);
        return lua_yieldk(L, 0, 0, present_k);
    }
    return 1;
}

/* ---- delay (behaviour of esp-claw lua_module_delay.c) ------------------ */

#define DELAY_PERIODIC_MT "delay.periodic"
#define DELAY_US_MAX_BLOCKING 1000000U

typedef struct {
    int64_t last_wake_us;
    int64_t period_us;
} delay_periodic_t;

static int delay_k(lua_State *L, int status, lua_KContext ctx)
{
    (void)status;
    lua_settop(L, (int)ctx);
    return (int)ctx;
}

static void busy_wait_us(int64_t us)
{
    int64_t until = mosaico_host_now_us() + us;
    while (mosaico_host_now_us() < until) {
    }
}

/* Sleeps by suspending the script (Request::Sleep). Where the script cannot
 * yield (inside a coroutine or a C callback) it blocks like vTaskDelay. The
 * results already on the stack (nret) are returned afterwards.
 * A stop request ends the wait early; the VM hook then stops the script in
 * a way pcall cannot catch (esp-claw raises a catchable "stop requested"). */
static int delay_wait(lua_State *L, int64_t ms, int nret)
{
    if (ms > 0 && !mosaico_lua_stop_requested(L) &&
        mosaico_lua_request(L, 1, ms > 0xffffffffLL ? 0xffffffffu : (uint32_t)ms))
        return lua_yieldk(L, 0, (lua_KContext)nret, delay_k);
    while (ms > 0 && !mosaico_lua_stop_requested(L)) {
        int64_t step = ms > 100 ? 100 : ms;
        busy_wait_us(step * 1000);
        ms -= step;
    }
    return nret;
}

static int delay_delay_ms(lua_State *L)
{
    lua_Integer ms = luaL_checkinteger(L, 1);
    if (ms < 0) ms = 0;
    if ((uint64_t)ms > UINT32_MAX) ms = UINT32_MAX;
    lua_settop(L, 0);
    return delay_wait(L, ms, 0);
}

static int delay_delay_us(lua_State *L)
{
    lua_Integer us = luaL_checkinteger(L, 1);
    if (us < 0) us = 0;
    if ((uint64_t)us > DELAY_US_MAX_BLOCKING)
        return luaL_error(L, "delay_us supports 0..%u only; use delay_ms for longer waits", DELAY_US_MAX_BLOCKING);
    busy_wait_us(us);
    return 0;
}

static int delay_periodic(lua_State *L)
{
    lua_Integer period_ms = luaL_checkinteger(L, 1);
    if (period_ms <= 0 || (uint64_t)period_ms > UINT32_MAX)
        return luaL_error(L, "period_ms must be in range 1..%u", UINT32_MAX);
    delay_periodic_t *p = (delay_periodic_t *)lua_newuserdata(L, sizeof(*p));
    p->last_wake_us = mosaico_host_now_us();
    p->period_us = (int64_t)period_ms * 1000;
    luaL_getmetatable(L, DELAY_PERIODIC_MT);
    lua_setmetatable(L, -2);
    return 1;
}

/* xTaskDelayUntil: the wake time always advances by one period; returns
 * false without waiting when that time has already passed. */
static int delay_periodic_wait(lua_State *L)
{
    delay_periodic_t *p = (delay_periodic_t *)luaL_checkudata(L, 1, DELAY_PERIODIC_MT);
    int64_t now = mosaico_host_now_us();
    int64_t wake = p->last_wake_us + p->period_us;
    p->last_wake_us = wake;
    lua_settop(L, 0);
    if (wake <= now) {
        lua_pushboolean(L, 0);
        return 1;
    }
    lua_pushboolean(L, 1);
    return delay_wait(L, (wake - now + 999) / 1000, 1);
}

static int delay_periodic_reset(lua_State *L)
{
    delay_periodic_t *p = (delay_periodic_t *)luaL_checkudata(L, 1, DELAY_PERIODIC_MT);
    p->last_wake_us = mosaico_host_now_us();
    return 0;
}

static int luaopen_delay(lua_State *L)
{
    if (luaL_newmetatable(L, DELAY_PERIODIC_MT)) {
        static const luaL_Reg methods[] = {
            {"wait", delay_periodic_wait},
            {"reset", delay_periodic_reset},
            {NULL, NULL},
        };
        lua_newtable(L);
        luaL_setfuncs(L, methods, 0);
        lua_setfield(L, -2, "__index");
    }
    lua_pop(L, 1);
    lua_newtable(L);
    lua_pushcfunction(L, delay_delay_ms);
    lua_setfield(L, -2, "delay_ms");
    lua_pushcfunction(L, delay_delay_us);
    lua_setfield(L, -2, "delay_us");
    lua_pushcfunction(L, delay_periodic);
    lua_setfield(L, -2, "periodic");
    return 1;
}

/* ---- system (per lua_module_system/README.md) ---------------------------- */

#define CAPS_DEFAULT MALLOC_CAP_DEFAULT

static void system_set_int(lua_State *L, const char *k, lua_Integer v)
{
    lua_pushinteger(L, v);
    lua_setfield(L, -2, k);
}

static double system_unix_seconds(void)
{
    return (double)mosaico_host_unix_us() / 1e6;
}

static int system_time(lua_State *L)
{
    lua_pushnumber(L, (lua_Number)(mosaico_host_unix_us() / 1000000));
    return 1;
}

static int system_format_date(lua_State *L, const char *fmt)
{
    if (strlen(fmt) > 64) return luaL_error(L, "date format too long");
    time_t t = (time_t)(mosaico_host_unix_us() / 1000000);
    struct tm tm;
    gmtime_r(&t, &tm);
    char buf[128];
    size_t n = strftime(buf, sizeof(buf), fmt, &tm);
    if (n == 0) return luaL_error(L, "date format produced empty string");
    lua_pushlstring(L, buf, n);
    return 1;
}

static int system_date(lua_State *L)
{
    return system_format_date(L, luaL_optstring(L, 1, "%Y-%m-%d %H:%M:%S"));
}

static int system_millis(lua_State *L)
{
    lua_pushnumber(L, (lua_Number)(mosaico_host_now_us() / 1000));
    return 1;
}

static int system_uptime(lua_State *L)
{
    lua_pushinteger(L, (lua_Integer)(mosaico_host_now_us() / 1000000));
    return 1;
}

static int system_ip(lua_State *L)
{
    lua_pushnil(L);
    return 1;
}

static int system_info(lua_State *L)
{
    mosaico_heap_info_t sram, psram;
    mosaico_host_heap_info(MALLOC_CAP_INTERNAL, &sram);
    mosaico_host_heap_info(MALLOC_CAP_SPIRAM, &psram);
    lua_newtable(L);
    system_set_int(L, "uptime_s", (lua_Integer)(mosaico_host_now_us() / 1000000));
    lua_pushnumber(L, (lua_Number)(mosaico_host_unix_us() / 1000000));
    lua_setfield(L, -2, "time");
    system_format_date(L, "%Y-%m-%d %H:%M:%S");
    lua_setfield(L, -2, "date");
    system_set_int(L, "sram_free", sram.free_size);
    system_set_int(L, "sram_total", sram.total_size);
    system_set_int(L, "sram_largest", sram.largest_free_block);
    if (psram.total_size) {
        system_set_int(L, "psram_free", psram.free_size);
        system_set_int(L, "psram_total", psram.total_size);
    }
    (void)system_unix_seconds;
    return 1;
}

static int system_heap_get_info(lua_State *L)
{
    lua_Integer caps = luaL_optinteger(L, 1, CAPS_DEFAULT);
    mosaico_heap_info_t info;
    mosaico_host_heap_info((uint32_t)caps, &info);
    lua_createtable(L, 0, 9);
    system_set_int(L, "caps", caps);
    system_set_int(L, "total_size", info.total_size);
    system_set_int(L, "free_size", info.free_size);
    system_set_int(L, "allocated_size", info.allocated_size);
    system_set_int(L, "minimum_free_size", info.minimum_free_size);
    system_set_int(L, "largest_free_block", info.largest_free_block);
    system_set_int(L, "allocated_blocks", info.allocated_blocks);
    system_set_int(L, "free_blocks", info.free_blocks);
    system_set_int(L, "total_blocks", info.total_blocks);
    return 1;
}

static int system_heap_get_task_watermarks(lua_State *L)
{
    lua_newtable(L);
    return 1;
}

static int system_heap_get_current_task(lua_State *L)
{
    lua_createtable(L, 0, 3);
    lua_pushstring(L, "lua");
    lua_setfield(L, -2, "name");
    system_set_int(L, "stack_high_water_mark_words", 0);
    system_set_int(L, "stack_high_water_mark_bytes", 0);
    return 1;
}

static int luaopen_system(lua_State *L)
{
    static const luaL_Reg fns[] = {
        {"time", system_time},     {"date", system_date}, {"millis", system_millis},
        {"uptime", system_uptime}, {"ip", system_ip},     {"info", system_info},
        {NULL, NULL},
    };
    luaL_newlib(L, fns);
    lua_createtable(L, 0, 4);
    lua_pushcfunction(L, system_heap_get_info);
    lua_setfield(L, -2, "get_info");
    lua_pushcfunction(L, system_heap_get_task_watermarks);
    lua_setfield(L, -2, "get_task_watermarks");
    lua_pushcfunction(L, system_heap_get_current_task);
    lua_setfield(L, -2, "get_current_task");
    lua_createtable(L, 0, 8);
    system_set_int(L, "EXEC", MALLOC_CAP_EXEC);
    system_set_int(L, "BIT32", MALLOC_CAP_32BIT);
    system_set_int(L, "BIT8", MALLOC_CAP_8BIT);
    system_set_int(L, "DMA", MALLOC_CAP_DMA);
    system_set_int(L, "SPIRAM", MALLOC_CAP_SPIRAM);
    system_set_int(L, "INTERNAL", MALLOC_CAP_INTERNAL);
    system_set_int(L, "DEFAULT", MALLOC_CAP_DEFAULT);
    lua_setfield(L, -2, "caps");
    lua_setfield(L, -2, "heap");
    return 1;
}

/* ---- storage root ------------------------------------------------------ */

/* storage.get_root_dir(): the writable data root (claw_paths CLAW_PATH_DATA). */
static char s_data_root[128] = "/data";

const char *mosaico_data_root(void) { return s_data_root; }

void mosaico_set_data_root(const char *path)
{
    snprintf(s_data_root, sizeof(s_data_root), "%s", path);
}

/* ---- preload ------------------------------------------------------------- */

extern const char mosaico_arg_schema_lua[];
extern const size_t mosaico_arg_schema_lua_len;

static int luaopen_arg_schema(lua_State *L)
{
    if (luaL_loadbufferx(L, mosaico_arg_schema_lua, mosaico_arg_schema_lua_len, "=arg_schema", "t") != LUA_OK)
        return lua_error(L);
    lua_call(L, 0, 1);
    return 1;
}

static void preload(lua_State *L, const char *name, lua_CFunction fn)
{
    lua_pushcfunction(L, fn);
    lua_setfield(L, -2, name);
}

void mosaico_platform_preload(lua_State *L)
{
    static bool registered;
    if (!registered) {
        registered = true;
        lua_module_display_register();
        lua_module_json_register();
        cap_lua_register_module("delay", luaopen_delay);
        cap_lua_register_module("system", luaopen_system);
        cap_lua_register_module("arg_schema", luaopen_arg_schema);
        lua_module_storage_register();
    }
    luaL_getsubtable(L, LUA_REGISTRYINDEX, LUA_PRELOAD_TABLE);
    for (size_t i = 0; i < s_module_count; ++i) preload(L, s_modules[i].name, s_modules[i].open);
    lua_pop(L, 1);
}
