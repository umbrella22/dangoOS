#pragma once
#define ESP_UNUSED(x) ((void)(x))
#define ESP_STATIC_ASSERT(c,m) _Static_assert(c,m)
#define unlikely(x) __builtin_expect(!!(x),0)
#define likely(x) __builtin_expect(!!(x),1)

#define BIT(n) (1U<<(n))
