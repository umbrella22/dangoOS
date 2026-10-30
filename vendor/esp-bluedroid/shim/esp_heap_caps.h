#pragma once
#include <stddef.h>
#include <stdlib.h>
#define MALLOC_CAP_8BIT (1<<2)
#define MALLOC_CAP_INTERNAL (1<<11)
#define MALLOC_CAP_SPIRAM (1<<10)
#define MALLOC_CAP_DEFAULT (1<<12)
#define heap_caps_free free
#define heap_caps_malloc mosaico_bt_heap_malloc
#define heap_caps_calloc mosaico_bt_heap_calloc
#define heap_caps_malloc_prefer mosaico_bt_heap_malloc_prefer
#define heap_caps_calloc_prefer mosaico_bt_heap_calloc_prefer
#define heap_caps_get_free_size mosaico_bt_heap_free_size
void *mosaico_bt_heap_malloc(size_t,unsigned);
void *mosaico_bt_heap_calloc(size_t,size_t,unsigned);
void *mosaico_bt_heap_malloc_prefer(size_t,size_t,...);
void *mosaico_bt_heap_calloc_prefer(size_t,size_t,size_t,...);
size_t mosaico_bt_heap_free_size(unsigned);
