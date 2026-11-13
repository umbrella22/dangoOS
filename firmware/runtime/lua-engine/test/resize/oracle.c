/* Frozen bilinear arithmetic from mosaico_image.c at 1314197adf97dc53.
 * Independent executable: reads decoded RGB and actual resize bytes, checks
 * the pre-optimization expression using the host C compiler's normal -O2
 * semantics, including its default FP contraction. No Lua or production helper
 * is linked into this reference. Records use four little-endian uint32 sizes,
 * sw*sh*3 source bytes, w*h*2 RGB565 bytes, then w*h GRAY8 bytes. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 |
           (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
static uint8_t *read_bytes(FILE *file, size_t n) {
    uint8_t *p = malloc(n);
    if (!p || fread(p, 1, n, file) != n) {
        fprintf(stderr, "invalid oracle record\n");
        exit(2);
    }
    return p;
}
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 2;
    unsigned cases = 0;
    uint8_t header[16];
    size_t got;
    while ((got = fread(header, 1, sizeof(header), file)) != 0) {
        if (got != sizeof(header)) return 2;
        unsigned sw = le32(header), sh = le32(header + 4);
        unsigned w = le32(header + 8), h = le32(header + 12);
        if (!sw || !sh || !w || !h || sw > 1920 || sh > 1080 ||
            w > 1920 || h > 1080) return 2;
        uint8_t *src = read_bytes(file, (size_t)sw * sh * 3);
        uint8_t *actual565 = read_bytes(file, (size_t)w * h * 2);
        uint8_t *actualgray = read_bytes(file, (size_t)w * h);
        for (unsigned y = 0; y < h; y++) for (unsigned x = 0; x < w; x++) {
            unsigned c[3];
            double sx=((x+0.5)*sw/w)-0.5,sy=((y+0.5)*sh/h)-0.5;if(sx<0)sx=0;if(sy<0)sy=0;if(sx>sw-1)sx=sw-1;if(sy>sh-1)sy=sh-1;
            unsigned x0=sx,y0=sy,x1=x0+1<sw?x0+1:x0,y1=y0+1<sh?y0+1:y0;double dx=sx-x0,dy=sy-y0;
            for(int k=0;k<3;k++)c[k]=(unsigned)(src[((size_t)y0*sw+x0)*3+k]*(1-dx)*(1-dy)+src[((size_t)y0*sw+x1)*3+k]*dx*(1-dy)+src[((size_t)y1*sw+x0)*3+k]*(1-dx)*dy+src[((size_t)y1*sw+x1)*3+k]*dx*dy+0.5);
            uint16_t v=((c[0]>>3)<<11)|((c[1]>>2)<<5)|(c[2]>>3);
            size_t i=(size_t)y*w+x;
            if (actual565[i*2] != (uint8_t)v || actual565[i*2+1] != v>>8 ||
                actualgray[i] != (uint8_t)((77*c[0]+150*c[1]+29*c[2]+128)>>8)) {
                fprintf(stderr,"case %u %ux%u -> %ux%u pixel %u,%u differs\n",cases,sw,sh,w,h,x,y);
                return 1;
            }
        }
        free(src); free(actual565); free(actualgray);
        cases++;
    }
    fclose(file);
    printf("%u cases: exact RGB565 and GRAY8\n",cases);
    return 0;
}
