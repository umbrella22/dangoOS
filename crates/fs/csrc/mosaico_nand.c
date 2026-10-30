/* Dhara ABI bridge. Chip operations and their error classification live in Rust. */
#include <stdlib.h>
#include <string.h>
#include "dhara/map.h"

struct mfs_nand {
    struct dhara_nand chip; /* first member: callbacks receive this address */
    struct dhara_map map;
    void *ctx;
    uint8_t *copy;
    uint8_t *journal;
};

extern int mosaico_nand_read(void *, uint32_t, size_t, size_t, uint8_t *);
extern int mosaico_nand_prog(void *, uint32_t, const uint8_t *);
extern int mosaico_nand_erase(void *, uint32_t);
extern int mosaico_nand_is_bad(void *, uint32_t);
extern void mosaico_nand_mark_bad(void *, uint32_t);
extern int mosaico_nand_is_free(void *, uint32_t);

static struct mfs_nand *owner(const struct dhara_nand *n) {
    return (struct mfs_nand *)n;
}
static int result(int code, dhara_error_t *err) {
    if (!code) return 0;
    /* Only chip-confirmed program/erase failures permit bad-block recovery.
       Transport/timeout failures are separately latched by the Rust context. */
    dhara_set_error(err, code == 1 ? DHARA_E_BAD_BLOCK : DHARA_E_ECC);
    return -1;
}
int dhara_nand_is_bad(const struct dhara_nand *n, dhara_block_t b) {
    return mosaico_nand_is_bad(owner(n)->ctx, b);
}
void dhara_nand_mark_bad(const struct dhara_nand *n, dhara_block_t b) {
    mosaico_nand_mark_bad(owner(n)->ctx, b);
}
int dhara_nand_is_free(const struct dhara_nand *n, dhara_page_t p) {
    return mosaico_nand_is_free(owner(n)->ctx, p);
}
int dhara_nand_erase(const struct dhara_nand *n, dhara_block_t b, dhara_error_t *err) {
    return result(mosaico_nand_erase(owner(n)->ctx, b), err);
}
int dhara_nand_prog(const struct dhara_nand *n, dhara_page_t p,
                    const uint8_t *data, dhara_error_t *err) {
    return result(mosaico_nand_prog(owner(n)->ctx, p, data), err);
}
int dhara_nand_read(const struct dhara_nand *n, dhara_page_t p,
                    size_t offset, size_t length, uint8_t *data, dhara_error_t *err) {
    return result(mosaico_nand_read(owner(n)->ctx, p, offset, length, data), err);
}
int dhara_nand_copy(const struct dhara_nand *n, dhara_page_t src,
                    dhara_page_t dst, dhara_error_t *err) {
    struct mfs_nand *m = owner(n);
    if (dhara_nand_read(n, src, 0, 1u << n->log2_page_size, m->copy, err)) return -1;
    return dhara_nand_prog(n, dst, m->copy, err);
}
void *mfs_nand_create(void *ctx, uint32_t blocks, uint8_t gc_ratio) {
    struct mfs_nand *m = calloc(1, sizeof(*m));
    if (!m) return NULL;
    m->journal = malloc(2048);
    m->copy = malloc(2048);
    if (!m->journal || !m->copy) {
        free(m->journal); free(m->copy); free(m); return NULL;
    }
    m->ctx = ctx;
    m->chip.log2_page_size = 11;
    m->chip.log2_ppb = 6;
    m->chip.num_blocks = blocks;
    dhara_map_init(&m->map, &m->chip, m->journal, gc_ratio);
    return m;
}
void mfs_nand_destroy(void *ptr) {
    struct mfs_nand *m = ptr;
    free(m->copy); free(m->journal); free(m);
}
int mfs_nand_resume(void *ptr) {
    dhara_error_t e = DHARA_E_NONE;
    return dhara_map_resume(&((struct mfs_nand *)ptr)->map, &e) ? -(int)e : 0;
}
uint32_t mfs_nand_capacity(void *ptr) {
    return dhara_map_capacity(&((struct mfs_nand *)ptr)->map);
}
int mfs_nand_read(void *ptr, uint32_t sector, uint8_t *data) {
    dhara_error_t e = DHARA_E_NONE;
    return dhara_map_read(&((struct mfs_nand *)ptr)->map, sector, data, &e) ? -(int)e : 0;
}
int mfs_nand_write(void *ptr, uint32_t sector, const uint8_t *data) {
    dhara_error_t e = DHARA_E_NONE;
    return dhara_map_write(&((struct mfs_nand *)ptr)->map, sector, data, &e) ? -(int)e : 0;
}
int mfs_nand_trim(void *ptr, uint32_t sector) {
    dhara_error_t e = DHARA_E_NONE;
    return dhara_map_trim(&((struct mfs_nand *)ptr)->map, sector, &e) ? -(int)e : 0;
}
int mfs_nand_sync(void *ptr) {
    dhara_error_t e = DHARA_E_NONE;
    return dhara_map_sync(&((struct mfs_nand *)ptr)->map, &e) ? -(int)e : 0;
}
