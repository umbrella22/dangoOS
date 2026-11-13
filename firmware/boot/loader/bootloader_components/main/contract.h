/* SPDX-License-Identifier: Apache-2.0
 * Byte contract shared with firmware/boot/contract. Never write a native struct.
 */
#pragma once
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#define DG_RECORD 100
#define DG_BODY 96
#define DG_MAIN_BASE 0x210000u
#define DG_MAIN_BYTES 0xc00000u
#define DG_RECOVERY_BASE 0x20000u
#define DG_RECOVERY_BYTES 0x1c0000u
enum dg_state { DG_RESCUE, DG_INSTALLING, DG_TRIAL, DG_TESTING, DG_READY, DG_STAGED };
struct dg_record { uint64_t generation; uint32_t state, image_bytes; uint8_t sha256[32]; };
static inline uint32_t dg_u32(const uint8_t *p) { return (uint32_t)p[0]|((uint32_t)p[1]<<8)|((uint32_t)p[2]<<16)|((uint32_t)p[3]<<24); }
static inline void dg_put32(uint8_t *p,uint32_t v) {for(int i=0;i<4;i++)p[i]=(uint8_t)(v>>(8*i));}
static inline uint32_t dg_crc(const uint8_t *p,unsigned n) {
    uint32_t c=~0u;for(unsigned i=0;i<n;i++){c^=p[i];for(int b=0;b<8;b++)c=(c>>1)^(0xedb88320u&(0u-(c&1)));}return ~c;
}
static inline bool dg_decode(const uint8_t *b,struct dg_record *r) {
    if(memcmp(b,"DGBOOT3\0",8)||memcmp(b+8,"S31ONE03",8)||memcmp(b+96,"DONE",4)||dg_crc(b,92)!=dg_u32(b+92))return false;
    for(int i=64;i<92;i++)if(b[i])return false;
    r->generation=(uint64_t)dg_u32(b+16)|((uint64_t)dg_u32(b+20)<<32);
    r->state=dg_u32(b+24);r->image_bytes=dg_u32(b+28);memcpy(r->sha256,b+32,32);
    return r->state<=DG_STAGED && (r->state==DG_RESCUE || (r->image_bytes && r->image_bytes<=DG_MAIN_BYTES));
}
static inline void dg_encode(uint8_t *b,const struct dg_record *r) {
    memset(b,0,DG_RECORD);memcpy(b,"DGBOOT3\0",8);memcpy(b+8,"S31ONE03",8);
    dg_put32(b+16,(uint32_t)r->generation);dg_put32(b+20,(uint32_t)(r->generation>>32));
    dg_put32(b+24,r->state);dg_put32(b+28,r->image_bytes);memcpy(b+32,r->sha256,32);
    dg_put32(b+92,dg_crc(b,92));memcpy(b+96,"DONE",4);
}
