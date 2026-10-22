/* The subset of esp-claw's cap_lua used by the vendored modules. */
#pragma once
#include <stddef.h>
#include <stdint.h>
#include "esp_err.h"
#include "lua.h"

#define CAP_LUA_JOB_ID_LEN 32

typedef void (*cap_lua_exit_cleanup_fn_t)(lua_State *L);

esp_err_t cap_lua_register_module(const char *name, lua_CFunction open_fn);
esp_err_t cap_lua_register_exit_cleanup(cap_lua_exit_cleanup_fn_t fn);
const char *cap_lua_runtime_job_id(lua_State *L);
esp_err_t cap_lua_stop_job(const char *job_id, uint32_t wait_ms, char *output, size_t output_size);
