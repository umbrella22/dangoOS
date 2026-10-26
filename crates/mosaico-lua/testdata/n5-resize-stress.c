/* Counterexample search against the frozen original C expression.
 * Host builds exercise large dimensions and 10,000 seeded random cases.
 * -DMOSAICO_BAREMETAL uses bounded small cases plus 65535-wide/tall axes,
 * static buffers and a Spike HTIF exit, with no libc or operating system. */
#include "../csrc/mosaico_resize.h"
#ifndef MOSAICO_BAREMETAL
#include <stdio.h>
#endif
#define CAP (1920u*1080u)
static uint8_t source[CAP*3], expected[CAP*2], actual[CAP*2];
static uint32_t seed=0x9367c54bu, cases, pixels;
static uint32_t random32(void) {
    seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
    return seed;
}
static int compare(uint32_t sw,uint32_t sh,uint32_t w,uint32_t h,int pattern) {
    if (!sw||!sh||!w||!h||sw>65535||sh>65535||w>65535||h>65535||sw*sh>CAP||w*h>CAP) return 2;
    for (size_t i=0;i<(size_t)sw*sh*3;i++)
        source[i]=pattern==0?(uint8_t)random32():pattern==1?255:pattern==2?(i&1?255:0):(uint8_t)i;
    for (int gray=0;gray<2;gray++) {
        for (unsigned y=0;y<h;y++) for(unsigned x=0;x<w;x++) {
            unsigned c[3];
            double sx=((x+0.5)*sw/w)-0.5,sy=((y+0.5)*sh/h)-0.5;if(sx<0)sx=0;if(sy<0)sy=0;if(sx>sw-1)sx=sw-1;if(sy>sh-1)sy=sh-1;
            unsigned x0=sx,y0=sy,x1=x0+1<sw?x0+1:x0,y1=y0+1<sh?y0+1:y0;double dx=sx-x0,dy=sy-y0;
            for(int k=0;k<3;k++)c[k]=(unsigned)(source[((size_t)y0*sw+x0)*3+k]*(1-dx)*(1-dy)+source[((size_t)y0*sw+x1)*3+k]*dx*(1-dy)+source[((size_t)y1*sw+x0)*3+k]*(1-dx)*dy+source[((size_t)y1*sw+x1)*3+k]*dx*dy+0.5);
            size_t at=(size_t)y*w+x;
            if(gray) expected[at]=(77*c[0]+150*c[1]+29*c[2]+128)>>8;
            else {uint16_t v=((c[0]>>3)<<11)|((c[1]>>2)<<5)|(c[2]>>3);expected[at*2]=v;expected[at*2+1]=v>>8;}
        }
        mosaico_resize_bilinear(source,sw,sh,actual,w,h,gray);
        for(size_t i=0;i<(size_t)w*h*(gray?1:2);i++) if(actual[i]!=expected[i]) {
#ifndef MOSAICO_BAREMETAL
            fprintf(stderr,"case=%u seed=%08x %ux%u -> %ux%u pattern=%d gray=%d byte=%zu expected=%u actual=%u\n",cases,seed,sw,sh,w,h,pattern,gray,i,expected[i],actual[i]);
#endif
            return 1;
        }
    }
    cases++;pixels+=w*h;
    return 0;
}
#define CHECK(sw,sh,w,h,p) do {if(compare(sw,sh,w,h,p)) return 1;} while(0)
static int run(void) {
    for (int p=0;p<4;p++) {
        CHECK(160,96,80,48,p);CHECK(160,96,81,48,p);
        CHECK(3,2,32,11,p);CHECK(257,5,65,7,p);
        CHECK(65535,1,257,3,p);CHECK(1,65535,3,257,p);
        CHECK(3,2,65535,1,p);CHECK(2,3,1,65535,p);
#ifndef MOSAICO_BAREMETAL
        CHECK(65535,31,117,73,p);CHECK(31,65535,73,117,p);
        CHECK(3,2,65535,31,p);CHECK(2,3,31,65535,p);
        CHECK(160,96,1920,1080,p);
#endif
    }
#ifdef MOSAICO_BAREMETAL
    const unsigned count=128,limit=33;
#else
    const unsigned count=10000,limit=97;
#endif
    for(unsigned i=0;i<count;i++) {
        uint32_t sw=1+random32()%limit,sh=1+random32()%limit;
        uint32_t w=1+random32()%limit,h=1+random32()%limit;
        CHECK(sw,sh,w,h,i%4);
    }
#ifndef MOSAICO_BAREMETAL
    printf("%u cases, %u output pixels per format: exact RGB565 and GRAY8\n",cases,pixels);
#endif
    return 0;
}
#ifdef MOSAICO_BAREMETAL
volatile uint64_t tohost __attribute__((section(".tohost"),aligned(8)));
volatile uint64_t fromhost __attribute__((section(".fromhost"),aligned(8)));
static uint8_t stack[16384] __attribute__((aligned(16),used));
void main(void) {
    int result=run();
    /* A zero exit is only possible after every expected case completed. */
    if(!result && cases!=160) result=2;
    tohost=((uint64_t)result<<1)|1;
    for(;;) __asm__ volatile("wfi");
}
__asm__(".section .text.init,\"ax\"\n.globl _start\n_start:\n"
        ".option push\n.option norelax\nla gp,__global_pointer$\n.option pop\n"
        "la sp,stack+16384\nli t0,0x2000\ncsrs mstatus,t0\ncall main\n1:j 1b\n");
#else
int main(void) {return run();}
#endif
