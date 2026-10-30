// SPDX-License-Identifier: Apache-2.0
// BR/EDR crypto ABI from ESP-IDF porting_btdm/controller/bredr/src/bredr.c.
// The controller uses little endian coordinates; TinyCrypt uses big endian.
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include "tinycrypt/ecc.h"
#include "tinycrypt/ecc_dh.h"
#include "tinycrypt/constants.h"
#include "tinycrypt/sha256.h"
#include "tinycrypt/hmac.h"
extern uint32_t mosaico_bredr_random(void);
static void erase(void *p,size_t n){volatile uint8_t *b=p;while(n--)*b++=0;}
static void reverse(uint8_t *out,const uint8_t *in,size_t n){for(size_t i=0;i<n;i++)out[i]=in[n-1-i];}
static int random_bytes(uint8_t *out,unsigned n){
    while(n){uint32_t word=mosaico_bredr_random();unsigned take=n<4?n:4;memcpy(out,&word,take);out+=take;n-=take;}
    return 1;
}
static int keypair(bool p256,uint8_t *pub,uint8_t *priv){
    size_t n=p256?32:24;uint8_t pk[64],sk[32];
    uECC_set_rng(random_bytes);
    int rc=uECC_make_key(pk,sk,p256?uECC_secp256r1():uECC_secp192r1());
    if(rc==TC_CRYPTO_SUCCESS){reverse(pub,pk,n);reverse(pub+n,pk+n,n);reverse(priv,sk,n);}
    erase(sk,sizeof(sk));return rc==TC_CRYPTO_SUCCESS?0:-1;
}
static int dhkey(bool p256,const uint8_t *x,const uint8_t *y,const uint8_t *priv,uint8_t *out){
    size_t n=p256?32:24;uint8_t pk[64],sk[32],dh[32];
    reverse(pk,x,n);reverse(pk+n,y,n);reverse(sk,priv,n);
    uECC_Curve curve=p256?uECC_secp256r1():uECC_secp192r1();
    // Reject invalid points, including zero and off-curve keys, before ECDH.
    int rc=-1;
    if(uECC_valid_public_key(pk,curve)==0){
        uECC_set_rng(random_bytes);
        if(uECC_shared_secret(pk,sk,dh,curve)==TC_CRYPTO_SUCCESS){reverse(out,dh,n);rc=0;}
    }
    erase(sk,sizeof(sk));erase(dh,sizeof(dh));return rc;
}
static int kp256(uint8_t *p,uint8_t *s){return keypair(true,p,s);}
static int kp192(uint8_t *p,uint8_t *s){return keypair(false,p,s);}
static int dh256(const uint8_t *x,const uint8_t *y,const uint8_t *s,uint8_t *o){return dhkey(true,x,y,s,o);}
static int dh192(const uint8_t *x,const uint8_t *y,const uint8_t *s,uint8_t *o){return dhkey(false,x,y,s,o);}
static int sha256(const uint8_t *msg,uint32_t n,uint8_t *out){
    struct tc_sha256_state_struct s;
    int ok=tc_sha256_init(&s)&&tc_sha256_update(&s,msg,n)&&tc_sha256_final(out,&s);
    erase(&s,sizeof(s));return ok?0:-1;
}
static int hmac(const uint8_t *key,uint32_t k,const uint8_t *msg,uint32_t n,uint8_t *out){
    struct tc_hmac_state_struct s;
    int ok=tc_hmac_set_key(&s,key,k)&&tc_hmac_init(&s)&&tc_hmac_update(&s,msg,n)&&tc_hmac_final(out,32,&s);
    erase(&s,sizeof(s));return ok?0:-1;
}
struct bredr_crypto_ops {
    struct {int (*kp256)(uint8_t*,uint8_t*);int (*dh256)(const uint8_t*,const uint8_t*,const uint8_t*,uint8_t*);
            int (*kp192)(uint8_t*,uint8_t*);int (*dh192)(const uint8_t*,const uint8_t*,const uint8_t*,uint8_t*);} ecc;
    struct {int (*sha)(const uint8_t*,uint32_t,uint8_t*);int (*hmac)(const uint8_t*,uint32_t,const uint8_t*,uint32_t,uint8_t*);} sha;
};
const struct bredr_crypto_ops MOSAICO_BREDR_CRYPTO_OPS={{kp256,dh256,kp192,dh192},{sha256,hmac}};
