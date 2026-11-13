#include <stdint.h>
#include <stddef.h>
extern void *mosaico_image_realloc(void *, size_t);
extern int mosaico_jpeg_decode_with_pool(const uint8_t *,size_t,uint16_t *,uint16_t *,uint8_t *,size_t,void *);
int mosaico_jpeg_decode_tracked(const uint8_t*d,size_t n,uint16_t*w,uint16_t*h,uint8_t*out,size_t cap) {
    void *pool=mosaico_image_realloc(NULL,4096);
    if (!pool) return 3;
    int result=mosaico_jpeg_decode_with_pool(d,n,w,h,out,cap,pool);
    mosaico_image_realloc(pool,0);
    return result;
}
