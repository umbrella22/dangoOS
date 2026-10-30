"""Run the actual BR/EDR crypto adapter against known points and hash vectors."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
VENDOR = ROOT / "vendor/esp-bluedroid"

HARNESS = r'''
#include <assert.h>
#include <stdio.h>
#include "crypto.c"
uint32_t mosaico_bredr_random(void) {
    /* Deterministic input only in this host test. Firmware uses hardware RNG. */
    static uint32_t x=0x12345678;
    x^=x<<13;x^=x>>17;x^=x<<5;return x;
}
static void hex(uint8_t *out,const char *s,size_t n) {
    for(size_t i=0;i<n;i++){unsigned b;assert(sscanf(s+i*2,"%2x",&b)==1);out[i]=(uint8_t)b;}
}
static void hash_vectors(void) {
    uint8_t out[32],expected[32],key[20];memset(key,0x0b,sizeof(key));
    assert(MOSAICO_BREDR_CRYPTO_OPS.sha.sha((const uint8_t*)"abc",3,out)==0);
    hex(expected,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",32);
    assert(memcmp(out,expected,32)==0);
    assert(MOSAICO_BREDR_CRYPTO_OPS.sha.hmac(key,20,(const uint8_t*)"Hi There",8,out)==0);
    hex(expected,"b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",32);
    assert(memcmp(out,expected,32)==0);
}
static void curve(bool p256,const char *gx,const char *gy,const char *four_x) {
    const size_t n=p256?32:24;uint8_t x[32],y[32],sk[32]={2},out[32],big[32],expected[32];
    hex(big,gx,n);reverse(x,big,n);hex(big,gy,n);reverse(y,big,n);
    const struct bredr_crypto_ops *ops=&MOSAICO_BREDR_CRYPTO_OPS;
    int (*dh)(const uint8_t*,const uint8_t*,const uint8_t*,uint8_t*)=p256?ops->ecc.dh256:ops->ecc.dh192;
    int (*kp)(uint8_t*,uint8_t*)=p256?ops->ecc.kp256:ops->ecc.kp192;
    /* 2G * 2 = 4G, independently checked with OpenSSL. TinyCrypt's ladder
       does not handle scalar 1 and its public-key check rejects G. */
    hex(big,four_x,n);reverse(expected,big,n);
    assert(dh(x,y,sk,out)==0);assert(memcmp(out,expected,n)==0);
    uint8_t bad[32]={0};assert(dh(bad,bad,sk,out)==-1);
    memset(bad,0xff,n);assert(dh(bad,y,sk,out)==-1);
    y[0]^=1;assert(dh(x,y,sk,out)==-1);
    uint8_t ap[64],as[32],bp[64],bs[32],a[32],b[32];
    assert(kp(ap,as)==0);assert(kp(bp,bs)==0);
    assert(dh(bp,bp+n,as,a)==0);assert(dh(ap,ap+n,bs,b)==0);
    assert(memcmp(a,b,n)==0);
}
int main(void) {
    hash_vectors();
    curve(false,"dafebf5828783f2ad35534631588a3f629a70fb16982a888",
                "dd6bda0d993da0fa46b27bbc141b868f59331afa5c7e93ab",
                "35433907297cc378b0015703374729d7a4fe46647084e4ba");
    curve(true,"7cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc47669978",
               "07775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1",
               "e2534a3532d08fbba02dde659ee62bd0031fe2db785596ef509302446b030852");
    puts("P192/P256 key agreement, invalid points, SHA256 and HMAC passed");
    return 0;
}
'''


class ClassicCryptoTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("cc"), "native C compiler required")
    def test_controller_crypto_vectors(self):
        with tempfile.TemporaryDirectory(prefix="mosaico-bredr-") as tmp:
            tmp = pathlib.Path(tmp)
            source = tmp / "vectors.c"
            source.write_text(HARNESS)
            binary = tmp / "vectors"
            tiny = VENDOR / "upstream/common/tinycrypt"
            command = ["cc", "-std=c11", "-O2", "-I", str(VENDOR / "src"),
                       "-I", str(VENDOR / "shim"), "-I", str(tiny / "include"),
                       str(source), *[str(tiny / "src" / f"{unit}.c") for unit in
                                      ("ecc", "ecc_dh", "sha256", "hmac", "utils")],
                       "-o", str(binary)]
            subprocess.run(command, check=True, capture_output=True, text=True)
            result = subprocess.run([str(binary)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("P192/P256 key agreement", result.stdout)
