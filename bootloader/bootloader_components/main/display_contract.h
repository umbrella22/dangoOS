/* SPDX-License-Identifier: Apache-2.0
 * Producer ABI exercised against crates/boot-contract by its C contract test.
 */
#pragma once
#include "contract.h"
#define DG_DISPLAY_FIRST_STORE 12u
#define DG_DISPLAY_MAGIC 0x31444744u
static inline void dg_display_record(uint32_t words[4], uint32_t first_us, uint32_t selected_us) {
    words[0] = DG_DISPLAY_MAGIC;
    words[1] = first_us;
    words[2] = selected_us;
    words[3] = dg_crc((const uint8_t *)words, 12);
}
