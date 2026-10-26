/* littlefs glue: one heap block per mounted file system, block device calls
 * routed to Rust (`mosaico_fs_bd_*`, `src/lfs.rs`). */
#include <stdlib.h>
#include <string.h>
#include "lfs.h"

extern int mosaico_fs_bd_read(void *ctx, lfs_block_t block, lfs_off_t off, void *buf, lfs_size_t size);
extern int mosaico_fs_bd_prog(void *ctx, lfs_block_t block, lfs_off_t off, const void *buf, lfs_size_t size);
extern int mosaico_fs_bd_erase(void *ctx, lfs_block_t block);
extern int mosaico_fs_bd_sync(void *ctx);

typedef struct {
    uint32_t read_size, prog_size, block_size, block_count;
    uint32_t cache_size, lookahead_size;
    int32_t block_cycles;
} mfs_geometry_t;

typedef struct {
    lfs_t lfs;
    struct lfs_config cfg;
} mfs_t;

static int bd_read(const struct lfs_config *c, lfs_block_t b, lfs_off_t o, void *buf, lfs_size_t n)
{
    return mosaico_fs_bd_read(c->context, b, o, buf, n);
}
static int bd_prog(const struct lfs_config *c, lfs_block_t b, lfs_off_t o, const void *buf, lfs_size_t n)
{
    return mosaico_fs_bd_prog(c->context, b, o, buf, n);
}
static int bd_erase(const struct lfs_config *c, lfs_block_t b)
{
    return mosaico_fs_bd_erase(c->context, b);
}
static int bd_sync(const struct lfs_config *c)
{
    return mosaico_fs_bd_sync(c->context);
}

mfs_t *mfs_create(void *ctx, const mfs_geometry_t *g)
{
    mfs_t *m = calloc(1, sizeof(*m));
    if (!m) return NULL;
    m->cfg.context = ctx;
    m->cfg.read = bd_read;
    m->cfg.prog = bd_prog;
    m->cfg.erase = bd_erase;
    m->cfg.sync = bd_sync;
    m->cfg.read_size = g->read_size;
    m->cfg.prog_size = g->prog_size;
    m->cfg.block_size = g->block_size;
    m->cfg.block_count = g->block_count;
    m->cfg.cache_size = g->cache_size;
    m->cfg.lookahead_size = g->lookahead_size;
    m->cfg.block_cycles = g->block_cycles;
    return m;
}

void mfs_destroy(mfs_t *m) { free(m); }
int mfs_mount(mfs_t *m) { return lfs_mount(&m->lfs, &m->cfg); }
int mfs_unmount(mfs_t *m) { return lfs_unmount(&m->lfs); }
int mfs_format(mfs_t *m) { return lfs_format(&m->lfs, &m->cfg); }
int mfs_mkdir(mfs_t *m, const char *p) { return lfs_mkdir(&m->lfs, p); }
int mfs_remove(mfs_t *m, const char *p) { return lfs_remove(&m->lfs, p); }
int mfs_rename(mfs_t *m, const char *a, const char *b) { return lfs_rename(&m->lfs, a, b); }
int mfs_make_consistent(mfs_t *m) { return lfs_fs_mkconsistent(&m->lfs); }
int32_t mfs_fs_size(mfs_t *m) { return lfs_fs_size(&m->lfs); }

/* type: 1 file, 2 dir (LFS_TYPE_REG / LFS_TYPE_DIR). */
int mfs_stat(mfs_t *m, const char *p, int *type, uint32_t *size)
{
    struct lfs_info info;
    int err = lfs_stat(&m->lfs, p, &info);
    if (err) return err;
    *type = info.type;
    *size = info.size;
    return 0;
}

int mfs_file_open(mfs_t *m, void **out, const char *p, int flags)
{
    lfs_file_t *f = calloc(1, sizeof(*f));
    if (!f) return LFS_ERR_NOMEM;
    int err = lfs_file_open(&m->lfs, f, p, flags);
    if (err) {
        free(f);
        return err;
    }
    *out = f;
    return 0;
}

int mfs_file_close(mfs_t *m, void *f)
{
    int err = lfs_file_close(&m->lfs, f);
    free(f);
    return err;
}

int32_t mfs_file_read(mfs_t *m, void *f, void *buf, uint32_t n) { return lfs_file_read(&m->lfs, f, buf, n); }
int32_t mfs_file_write(mfs_t *m, void *f, const void *buf, uint32_t n) { return lfs_file_write(&m->lfs, f, buf, n); }
int32_t mfs_file_seek(mfs_t *m, void *f, int32_t off, int whence) { return lfs_file_seek(&m->lfs, f, off, whence); }
int32_t mfs_file_size(mfs_t *m, void *f) { return lfs_file_size(&m->lfs, f); }
int mfs_file_sync(mfs_t *m, void *f) { return lfs_file_sync(&m->lfs, f); }
int mfs_file_truncate(mfs_t *m, void *f, uint32_t size) { return lfs_file_truncate(&m->lfs, f, size); }

int mfs_dir_open(mfs_t *m, void **out, const char *p)
{
    lfs_dir_t *d = calloc(1, sizeof(*d));
    if (!d) return LFS_ERR_NOMEM;
    int err = lfs_dir_open(&m->lfs, d, p);
    if (err) {
        free(d);
        return err;
    }
    *out = d;
    return 0;
}

/* Returns 1 with an entry, 0 at the end, < 0 on error. `name` holds
 * LFS_NAME_MAX + 1 bytes. */
int mfs_dir_read(mfs_t *m, void *d, int *type, uint32_t *size, char *name)
{
    struct lfs_info info;
    int r = lfs_dir_read(&m->lfs, d, &info);
    if (r <= 0) return r;
    *type = info.type;
    *size = info.size;
    memcpy(name, info.name, sizeof(info.name));
    return 1;
}

int mfs_dir_close(mfs_t *m, void *d)
{
    int err = lfs_dir_close(&m->lfs, d);
    free(d);
    return err;
}
