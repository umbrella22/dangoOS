/* Baseline JPEG to RGB888 with TJpgDec (launcher icons, `icon.jpg`). */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "tjpgd.h"

/* TJpgDec R0.03 with JD_FASTDECODE 1 needs about 3.1 KiB of work area. */
#define POOL_SIZE 4096

typedef struct {
    const uint8_t *data;
    size_t len;
    size_t pos;
    uint8_t *out;
    uint16_t width;
} source_t;

static size_t in_func(JDEC *jd, uint8_t *buf, size_t n)
{
    source_t *s = (source_t *)jd->device;
    size_t left = s->len - s->pos;
    if (n > left) n = left;
    if (buf) memcpy(buf, s->data + s->pos, n);
    s->pos += n;
    return n;
}

static int out_func(JDEC *jd, void *bitmap, JRECT *rect)
{
    source_t *s = (source_t *)jd->device;
    const uint8_t *src = (const uint8_t *)bitmap;
    size_t row = (size_t)(rect->right - rect->left + 1) * 3;
    for (unsigned y = rect->top; y <= rect->bottom; y++) {
        memcpy(s->out + ((size_t)y * s->width + rect->left) * 3, src, row);
        src += row;
    }
    return 1;
}

/* Decodes `data` into `out` (width * height * 3 bytes, RGB888). With
 * `out == NULL` only reads the size. Returns 0 or the TJpgDec error code. */
extern void *mosaico_image_realloc(void*,size_t);
static int decode(const uint8_t *data, size_t len, uint16_t *width, uint16_t *height,
                        uint8_t *out, size_t out_len,int tracked)
{
    void *pool = tracked ? mosaico_image_realloc(NULL,POOL_SIZE) : malloc(POOL_SIZE);
    if (!pool) return JDR_MEM1;
    source_t s = {.data = data, .len = len};
    JDEC jd;
    JRESULT r = jd_prepare(&jd, in_func, pool, POOL_SIZE, &s);
    if (r == JDR_OK) {
        *width = jd.width;
        *height = jd.height;
        if (out) {
            if (out_len < (size_t)jd.width * jd.height * 3) {
                r = JDR_PAR;
            } else {
                s.out = out;
                s.width = jd.width;
                r = jd_decomp(&jd, out_func, 0);
            }
        }
    }
    if(tracked)mosaico_image_realloc(pool,0);else free(pool);
    return (int)r;
}

int mosaico_jpeg_decode(const uint8_t*d,size_t n,uint16_t*w,uint16_t*h,uint8_t*out,size_t cap){return decode(d,n,w,h,out,cap,0);}
int mosaico_jpeg_decode_tracked(const uint8_t*d,size_t n,uint16_t*w,uint16_t*h,uint8_t*out,size_t cap){return decode(d,n,w,h,out,cap,1);}
