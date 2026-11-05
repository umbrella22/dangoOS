/* Private bilinear kernel. Inputs obey image's 65535-dimension and
 * 1920*1080-pixel limits. No allocation or dependency on Lua is needed. */
#ifndef MOSAICO_RESIZE_H
#define MOSAICO_RESIZE_H
#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint32_t first, second, fraction;
    int binary;
} mosaico_resize_axis;

static mosaico_resize_axis mosaico_resize_sample(uint32_t i, uint32_t source,
                                                 uint32_t target, uint32_t odd) {
    uint32_t den = 2 * target;
    int64_t numerator = (int64_t)(2 * i + 1) * source - target;
    mosaico_resize_axis a;
    if (numerator <= 0) {
        a.first = a.fraction = 0;
    } else if (numerator >= (int64_t)(source - 1) * den) {
        a.first = source - 1;
        a.fraction = 0;
    } else {
        a.first = numerator / den;
        a.fraction = numerator % den;
    }
    a.second = a.first + 1 < source ? a.first + 1 : a.first;
    /* Removing factors of two leaves the odd denominator. A fraction is
     * exactly binary precisely when its numerator cancels this odd part. */
    a.binary = a.fraction % odd == 0;
    return a;
}

static void mosaico_resize_bilinear(const uint8_t *src, uint32_t sw, uint32_t sh,
                                    uint8_t *out, uint32_t w, uint32_t h, int gray) {
    uint32_t dxden = 2 * w, dyden = 2 * h, den = dxden * dyden;
    uint32_t xodd = w, yodd = h;
    while (!(xodd & 1)) xodd >>= 1;
    while (!(yodd & 1)) yodd >>= 1;
    mosaico_resize_axis axis[32];
    for (uint32_t base = 0; base < w; base += 32) {
        uint32_t count = w - base < 32 ? w - base : 32;
        for (uint32_t j = 0; j < count; j++)
            axis[j] = mosaico_resize_sample(base + j, sw, w, xodd);
        for (uint32_t y = 0; y < h; y++) {
            mosaico_resize_axis ay = mosaico_resize_sample(y, sh, h, yodd);
            for (uint32_t j = 0; j < count; j++) {
                mosaico_resize_axis ax = axis[j];
                uint32_t x = base + j;
                uint32_t weights[4] = {
                    (dxden - ax.fraction) * (dyden - ay.fraction),
                    ax.fraction * (dyden - ay.fraction),
                    (dxden - ax.fraction) * ay.fraction,
                    ax.fraction * ay.fraction,
                };
                size_t offsets[4] = {
                    ((size_t)ay.first * sw + ax.first) * 3,
                    ((size_t)ay.first * sw + ax.second) * 3,
                    ((size_t)ay.second * sw + ax.first) * 3,
                    ((size_t)ay.second * sw + ax.second) * 3,
                };
                uint32_t c[3];
                int legacy_ready = 0;
                double dx = 0, dy = 0;
                for (int k = 0; k < 3; k++) {
                    /* D=4*w*h <= 8,294,400; sum <= 255*D and even
                     * sum+D/2 <= 2,119,219,200. Every operation fits u32. */
                    uint32_t sum = src[offsets[0]+k] * weights[0] +
                                   src[offsets[1]+k] * weights[1] +
                                   src[offsets[2]+k] * weights[2] +
                                   src[offsets[3]+k] * weights[3];
                    uint32_t rounded = sum + den / 2;
                    c[k] = rounded / den;
                    /* The old double coordinates differ from the rational
                     * coordinates by <= 2^-37 per axis (product exact; one
                     * division/subtraction). Including interpolation rounding,
                     * the channel error is < 2^-24, while adjacent rational
                     * values are >= 1/D > 2^-24 apart. Thus only an EXACT half
                     * can round differently. Binary weights are exact in double
                     * throughout (integer numerators <= 255*D < 2^31).
                     * For nonbinary ties retain the original expression/order. */
                    if (!(ax.binary && ay.binary) && rounded == c[k] * den) {
                        if (!legacy_ready) {
                            double sx=((x+0.5)*sw/w)-0.5, sy=((y+0.5)*sh/h)-0.5;
                            if(sx<0)sx=0;if(sy<0)sy=0;if(sx>sw-1)sx=sw-1;if(sy>sh-1)sy=sh-1;
                            dx=sx-ax.first;dy=sy-ay.first;
                            legacy_ready = 1;
                        }
                        c[k]=(unsigned)(src[offsets[0]+k]*(1-dx)*(1-dy)+src[offsets[1]+k]*dx*(1-dy)+src[offsets[2]+k]*(1-dx)*dy+src[offsets[3]+k]*dx*dy+0.5);
                    }
                }
                size_t at = (size_t)y * w + x;
                if (gray) out[at] = (77*c[0]+150*c[1]+29*c[2]+128)>>8;
                else {
                    uint16_t v=((c[0]>>3)<<11)|((c[1]>>2)<<5)|(c[2]>>3);
                    out[at*2]=v;out[at*2+1]=v>>8;
                }
            }
        }
    }
}
#endif
