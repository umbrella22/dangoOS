#pragma once
#include <stdint.h>
#define ntohs(x) __builtin_bswap16((uint16_t)(x))
#define htons(x) ntohs(x)
#define ntohl(x) __builtin_bswap32((uint32_t)(x))
#define htonl(x) ntohl(x)
