/* The Lua runtime is single threaded: one script, one task. These stubs keep
 * the vendored module code unchanged. */
#pragma once
#include <stdbool.h>
#include <stdint.h>

typedef int BaseType_t;
typedef uint32_t TickType_t;
#define pdTRUE 1
#define pdFALSE 0
#define pdPASS 1
#define pdFAIL 0
#define portMAX_DELAY 0xffffffffu
