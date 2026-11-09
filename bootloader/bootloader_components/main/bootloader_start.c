/* SPDX-License-Identifier: Apache-2.0
 * Independent Recovery and one main image. No OTA-slot sequence selection.
 * Uses ESP-IDF's flash initialization, image verifier, cache setup and jump.
 */
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include "sdkconfig.h"
#include "bootloader_init.h"
#include "bootloader_utility.h"
#include "bootloader_common.h"
#include "bootloader_sha_flash.h"
#include "esp_private/bootloader_flash_internal.h"
#include "esp_image_format.h"
#include "contract.h"
#include "display.h"

static bool read_record(unsigned i,struct dg_record *r) {
    uint32_t words[DG_RECORD/4];
    return bootloader_flash_read(0x9000u+i*4096u,words,sizeof(words),false)==ESP_OK && dg_decode((const uint8_t*)words,r);
}
static int journal(struct dg_record *record) {
    struct dg_record a,b;bool va=read_record(0,&a),vb=read_record(1,&b);
    if(va&&vb&&a.generation==b.generation && (a.state!=b.state || a.image_bytes!=b.image_bytes || memcmp(a.sha256,b.sha256,32)))return -1;
    if(vb&&(!va||b.generation>a.generation)){*record=b;return 1;}
    if(va){*record=a;return 0;}return -1;
}
static bool consume_trial(int active,struct dg_record *r) {
    if(r->generation==UINT64_MAX)return false;
    r->generation++;r->state=DG_TESTING;
    uint32_t words[DG_RECORD/4],verify[DG_RECORD/4];dg_encode((uint8_t*)words,r);
    uint32_t address=0x9000u+(1u-(unsigned)active)*4096u;
    if(bootloader_flash_erase_range(address,4096)!=ESP_OK || bootloader_flash_write(address,words,DG_BODY,false)!=ESP_OK)return false;
    if(bootloader_flash_read(address,verify,DG_RECORD,false)!=ESP_OK || memcmp(words,verify,DG_BODY) || verify[DG_BODY/4]!=UINT32_MAX)return false;
    if(bootloader_flash_write(address+DG_BODY,&words[DG_BODY/4],4,false)!=ESP_OK)return false;
    struct dg_record decoded;
    return bootloader_flash_read(address,verify,DG_RECORD,false)==ESP_OK && dg_decode((const uint8_t*)verify,&decoded) && decoded.generation==r->generation && decoded.state==DG_TESTING;
}
static bool valid_main(const struct dg_record *r) {
    uint8_t digest[32];esp_image_metadata_t image={0};
    esp_partition_pos_t pos={.offset=DG_MAIN_BASE,.size=DG_MAIN_BYTES};
    return r->image_bytes>0 && r->image_bytes<=DG_MAIN_BYTES &&
        bootloader_sha256_flash_contents(pos.offset,r->image_bytes,digest)==ESP_OK && !memcmp(digest,r->sha256,32) &&
        esp_image_verify(ESP_IMAGE_VERIFY,&pos,&image)==ESP_OK;
}
static bool strict_layout(void) {
    static const struct { const char *name; uint8_t type, subtype; uint32_t offset, bytes; } expected[] = {
        {"boot_state",1,0x40,0x9000,0x2000}, {"phy_init",1,1,0xb000,0x1000},
        {"sysmeta",1,2,0xc000,0x14000}, {"recovery_app",0,0,DG_RECOVERY_BASE,DG_RECOVERY_BYTES},
        {"coredump",1,3,0x1e0000,0x20000}, {"nvs",1,2,0x200000,0x6000},
        {"main_app",0,0x10,DG_MAIN_BASE,DG_MAIN_BYTES}, {"system",1,0x83,0xe10000,0x1f0000}
    };
    uint32_t words[256/4];
    if(bootloader_flash_read(0x8000,words,sizeof(words),false)!=ESP_OK)return false;
    const uint8_t *table=(const uint8_t*)words;
    for(unsigned i=0;i<sizeof(expected)/sizeof(expected[0]);i++) {
        const uint8_t *e=table+i*32;
        uint8_t label[16]={0};memcpy(label,expected[i].name,strlen(expected[i].name));
        if(e[0]!=0xaa || e[1]!=0x50 || e[2]!=expected[i].type || e[3]!=expected[i].subtype ||
            dg_u32(e+4)!=expected[i].offset || dg_u32(e+8)!=expected[i].bytes ||
            memcmp(e+12,label,16) || dg_u32(e+28))return false;
    }
    return true;
}
void __attribute__((noreturn)) call_start_cpu0(void) {
    if(bootloader_init()!=ESP_OK)bootloader_reset();
    dg_display_begin();
    bootloader_state_t partitions={0};struct dg_record record;
    int active=journal(&record);
    bool layout=bootloader_utility_load_partition_table(&partitions) && strict_layout() && partitions.app_count==1 &&
        partitions.factory.offset==DG_RECOVERY_BASE && partitions.factory.size==DG_RECOVERY_BYTES &&
        partitions.ota[0].offset==DG_MAIN_BASE && partitions.ota[0].size==DG_MAIN_BYTES;
    // AI is GPIO7. BOOT is the ROM download strap and remains the ROM entry.
    bool manual=bootloader_common_check_long_hold_gpio_level(7,1,false)==GPIO_LONG_HOLD;
    bool main= !manual && layout && active>=0 && (record.state==DG_READY || record.state==DG_TRIAL) && valid_main(&record);
    if(main && record.state==DG_TRIAL)main=consume_trial(active,&record);
    // TEST_APP_INDEX loads exactly the supplied position. The standard loader's
    // OTA/factory search must not revive a half-installed main image.
    bootloader_state_t selected={0};
    selected.test.offset=main?DG_MAIN_BASE:DG_RECOVERY_BASE;
    selected.test.size=main?DG_MAIN_BYTES:DG_RECOVERY_BYTES;
    dg_display_handoff();
    bootloader_utility_load_boot_image(&selected,TEST_APP_INDEX);
    bootloader_reset();
}
