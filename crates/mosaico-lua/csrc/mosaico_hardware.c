/* Snapshot-only board adapters for esp-claw's public Lua contracts.
 * No Lua error crosses a Rust frame: host calls return before luaL_error. */
#include <math.h>
#include <stdint.h>
#include <string.h>

#include "lauxlib.h"
#include "lua.h"
#include "mosaico_hardware.h"

/* Keep in sync with src/hardware.rs. All fields have four-byte alignment. */
typedef struct {
    uint32_t sequence, at_ms, key, pressed;
} edge_t;

typedef struct {
    uint32_t available, valid;
    int32_t accel[3], gyro[3], temperature;
    uint32_t sensor_time, imu_status;
    float magnetic[3];
    uint32_t magnet_status, magnet_state, voltage_mv;
    int32_t current_ma;
    uint32_t soc, charging;
    int32_t button_pins[2];
    uint32_t button_pressed[2], led_generation, button_generation, button_sequence;
    edge_t button_edges[16];
} snapshot_t;

extern void mosaico_host_hardware_snapshot(snapshot_t *);
extern int mosaico_host_hardware_request(uint32_t, const uint8_t *);
extern int64_t mosaico_host_now_us(void);
extern const char mosaico_hardware_lua[];
extern const size_t mosaico_hardware_lua_len;

static void number(lua_State *L, const char *key, double value)
{
    lua_pushnumber(L, value);
    lua_setfield(L, -2, key);
}

static void vector(lua_State *L, const char *key, const double *values, int count)
{
    lua_createtable(L, count, 0);
    for (int i = 0; i < count; i++) {
        lua_pushnumber(L, values[i]);
        lua_rawseti(L, -2, i + 1);
    }
    lua_setfield(L, -2, key);
}

static int snapshot(lua_State *L)
{
    snapshot_t s;
    mosaico_host_hardware_snapshot(&s);
    lua_newtable(L);
    number(L, "available", s.available);
    number(L, "valid", s.valid);

    double values[3];
    for (int i = 0; i < 3; i++) values[i] = s.accel[i];
    vector(L, "accel", values, 3);
    for (int i = 0; i < 3; i++) values[i] = s.gyro[i];
    vector(L, "gyro", values, 3);
    for (int i = 0; i < 3; i++) values[i] = s.magnetic[i];
    vector(L, "magnetic", values, 3);

    number(L, "temperature", s.temperature);
    number(L, "sensor_time", s.sensor_time);
    number(L, "imu_status", s.imu_status);
    number(L, "magnet_status", s.magnet_status);
    number(L, "magnet_state", s.magnet_state);
    number(L, "voltage_mv", s.voltage_mv);
    number(L, "current_ma", s.current_ma);
    number(L, "soc", s.soc);
    number(L, "charging", s.charging);
    for (int i = 0; i < 2; i++) values[i] = s.button_pins[i];
    vector(L, "pins", values, 2);
    for (int i = 0; i < 2; i++) values[i] = s.button_pressed[i];
    vector(L, "pressed", values, 2);
    number(L, "led_generation", s.led_generation);
    number(L, "generation", s.button_generation);
    number(L, "sequence", s.button_sequence);

    lua_createtable(L, 16, 0);
    for (int i = 0; i < 16; i++) {
        lua_newtable(L);
        number(L, "sequence", s.button_edges[i].sequence);
        number(L, "at", s.button_edges[i].at_ms);
        number(L, "key", s.button_edges[i].key + 1);
        number(L, "pressed", s.button_edges[i].pressed);
        lua_rawseti(L, -2, i + 1);
    }
    lua_setfield(L, -2, "edges");
    return 1;
}

/* Strict argument schema at the C boundary. The official arg_schema Lua
 * helper normalizes/clamps, which is inappropriate for GPIO/range validation.
 * Whole-valued Lua floats are accepted, but strings and fractions are not. */
static int integer(lua_State *L)
{
    int exact;
    lua_Integer value = lua_tointegerx(L, 1, &exact);
    if (lua_type(L, 1) != LUA_TNUMBER || !exact) {
        return luaL_error(L, "expected integer");
    }
    lua_Integer min = luaL_checkinteger(L, 2);
    lua_Integer max = luaL_checkinteger(L, 3);
    if (value < min || value > max) {
        return luaL_error(L, "integer outside allowed range");
    }
    lua_pushinteger(L, value);
    return 1;
}

static int check_finite(lua_State *L)
{
    if (lua_type(L, 1) != LUA_TNUMBER || !isfinite(lua_tonumber(L, 1))) {
        return luaL_error(L, "expected finite number");
    }
    lua_settop(L, 1);
    return 1;
}

static int request(lua_State *L)
{
    int kind = (int)luaL_checkinteger(L, 1);
    uint8_t data[18] = {0};
    if (kind == 0) {
        lua_Integer preset = luaL_checkinteger(L, 2);
        if (preset < 0 || preset >= 13) {
            return luaL_error(L, "unknown haptic preset");
        }
        data[0] = (uint8_t)preset;
    } else if (kind == 1) {
        size_t len;
        const char *pixels = luaL_checklstring(L, 2, &len);
        if (len != sizeof(data)) {
            return luaL_error(L, "expected six RGB pixels");
        }
        memcpy(data, pixels, sizeof(data));
    } else if (kind != 2) {
        return luaL_error(L, "unknown hardware request");
    }
    int error = mosaico_host_hardware_request(kind, data);
    if (error) {
        return luaL_error(L, "hardware request failed: %d", error);
    }
    return 0;
}

static int now(lua_State *L)
{
    lua_pushinteger(L, mosaico_host_now_us() / 1000);
    return 1;
}

int luaopen_mosaico_hardware(lua_State *L)
{
    if (luaL_loadbuffer(L, mosaico_hardware_lua, mosaico_hardware_lua_len,
                       "@mosaico/hardware.lua") != LUA_OK) {
        return lua_error(L);
    }
    lua_newtable(L);
    const luaL_Reg functions[] = {
        {"snapshot", snapshot}, {"integer", integer}, {"finite", check_finite},
        {"request", request}, {"now", now}, {NULL, NULL}
    };
    luaL_setfuncs(L, functions, 0);
    lua_call(L, 1, 1);
    return 1;
}

static int open_module(lua_State *L)
{
    const char *name = lua_tostring(L, lua_upvalueindex(1));
    lua_getglobal(L, "require");
    lua_pushliteral(L, "_mosaico_hardware");
    lua_call(L, 1, 1);
    lua_getfield(L, -1, name);
    return 1;
}

void mosaico_hardware_preload(lua_State *L)
{
    lua_getglobal(L, "package");
    lua_getfield(L, -1, "preload");
    lua_pushcfunction(L, luaopen_mosaico_hardware);
    lua_setfield(L, -2, "_mosaico_hardware");
    const char *names[] = {
        "imu", "magnetometer", "fuel_gauge", "lib_fuel_gauge", "haptic",
        "led_strip", "button", NULL
    };
    for (int i = 0; names[i]; i++) {
        lua_pushstring(L, names[i]);
        lua_pushcclosure(L, open_module, 1);
        lua_setfield(L, -2, names[i]);
    }
    lua_pop(L, 2);
}
