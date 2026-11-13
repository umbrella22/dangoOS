/* esp-claw claw_paths for lua_module_storage: the only path is the writable
 * data root, "/data" on the device (settable for host tests). */
#pragma once
#include <stdint.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include "esp_err.h"

#ifndef __GLIBC__
/* newlib declares lstat only for some feature-test macros; fs
 * defines it (there are no symlinks: it is stat). */
int lstat(const char *path, struct stat *st);
#endif

typedef enum { CLAW_PATH_DATA = 0 } claw_path_t;

const char *mosaico_data_root(void);

static inline const char *claw_paths_get(claw_path_t which)
{
    (void)which;
    return mosaico_data_root();
}

static inline esp_err_t claw_paths_get_space(claw_path_t which, uint64_t *total, uint64_t *free_bytes)
{
    struct statvfs st;
    if (statvfs(claw_paths_get(which), &st) != 0) return ESP_FAIL;
    *total = (uint64_t)st.f_blocks * st.f_frsize;
    *free_bytes = (uint64_t)st.f_bavail * st.f_frsize;
    return ESP_OK;
}
