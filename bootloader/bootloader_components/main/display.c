/* SPDX-License-Identifier: Apache-2.0
 * Minimal CO5300 splash: polling SPI2 FIFO, no heap, PSRAM, DMA or filesystem.
 * The application's DMA driver later reclaims SPI2 while retaining panel GRAM.
 */
#include <stdbool.h>
#include <stdint.h>
#include "display.h"
#include "contract.h"
#include "display_contract.h"
#include "esp_cpu.h"
#include "esp_rom_sys.h"
#include "esp_rom_gpio.h"
#include "hal/gpio_ll.h"
#include "hal/spi_ll.h"
#include "soc/gpio_sig_map.h"
#include "soc/lp_system_reg.h"
#include "splash.inc"

#define STORE(i) (LP_SYSTEM_REG_LP_STORE12_REG + (i) * 4)
static uint32_t started, first_us;
static bool painted;

static uint32_t elapsed_us(void) {
    return (esp_cpu_get_cycle_count() - started) / esp_rom_get_cpu_ticks_per_us();
}

static void output(unsigned pin, unsigned level) {
    esp_rom_gpio_pad_select_gpio(pin);
    esp_rom_gpio_connect_out_signal(pin, SIG_GPIO_OUT_IDX, false, false);
    gpio_ll_set_level(&GPIO, pin, level);
    gpio_ll_output_enable(&GPIO, pin);
}

static bool wait_clear(volatile uint32_t *reg, uint32_t mask) {
    for (unsigned n = 0; n < 100000; n++) {
        if (!(*reg & mask)) return true;
    }
    return false;
}

static bool transfer(uint8_t operation, uint32_t address, const uint8_t *bytes,
                     unsigned count, bool quad, bool prefix, bool keep_cs) {
    spi_dev_t *hw = &GPSPI2;
    spi_ll_master_set_line_mode(hw, (spi_line_mode_t){.cmd_lines=1, .addr_lines=1, .data_lines=quad?4:1});
    spi_ll_set_command_bitlen(hw, prefix?8:0);
    spi_ll_set_addr_bitlen(hw, prefix?24:0);
    if (prefix) {
        spi_ll_set_command(hw, operation, 8, false);
        spi_ll_set_address(hw, address, 24, false);
    }
    spi_ll_enable_mosi(hw, count != 0);
    if (count) {
        uint32_t buffer[16] = {0};
        memcpy(buffer, bytes, count);
        spi_ll_write_buffer(hw, (const uint8_t *)buffer, count*8);
        spi_ll_set_mosi_bitlen(hw, count*8);
    }
    spi_ll_master_keep_cs(hw, keep_cs);
    hw->cmd.update = 1;
    if (!wait_clear(&hw->cmd.val, SPI_UPDATE)) return false;
    spi_ll_user_start(hw);
    return wait_clear(&hw->cmd.val, SPI_USR);
}

static bool reg(uint8_t command, const uint8_t *data, unsigned n) {
    return transfer(0x02, (uint32_t)command << 8, data, n, false, true, false);
}

static bool byte(uint8_t command, uint8_t value) {
    return reg(command, &value, 1);
}

static bool paint(void) {
    const uint8_t window[] = {0, 0, 1, 0xdf};
    if (!reg(0x2a, window, 4) || !reg(0x2b, window, 4)) return false;
    uint8_t fifo[64];
    unsigned used = 0, pixels = 0;
    bool prefix = true;
    for (unsigned at = 0; at < sizeof(dg_splash_runs);) {
        uint8_t code = dg_splash_runs[at++];
        uint32_t count = (code & 15) + 1;
        if (count == 16) {
            unsigned shift = 0;
            uint8_t extension;
            do {
                if (at == sizeof(dg_splash_runs) || shift > 21) return false;
                extension = dg_splash_runs[at++];
                count += (uint32_t)(extension & 127) << shift;
                shift += 7;
            } while (extension & 128);
        }
        if (count > 480u*480u - pixels) return false;
        uint16_t gray = (code >> 4)*17u;
        uint16_t color = ((gray >> 3) << 11) | ((gray >> 2) << 5) | (gray >> 3);
        for (uint32_t n = 0; n < count; n++) {
            fifo[used++] = color >> 8;
            fifo[used++] = color;
            pixels++;
            if (used == sizeof(fifo)) {
                if (!transfer(0x32, 0x2c00, fifo, used, true, prefix, pixels != 480u*480u)) return false;
                used = 0;
                prefix = false;
            }
        }
    }
    return pixels == 480u*480u && used == 0;
}

void dg_display_begin(void) {
    REG_WRITE(STORE(0), 0);
    painted = false;
    started = esp_cpu_get_cycle_count();
    output(60, 0); // Shared 3.3 V rail, active low.
    output(50, 1);
    output(44, 0);
    esp_rom_delay_us(10000);
    output(44, 1);
    esp_rom_delay_us(150000);

    // No interrupt/executor runs in this bootloader; the LL clock macros still
    // require their explicit RCC atomic-environment marker.
    int __DECLARE_RCC_ATOMIC_ENV = 0;
    spi_ll_enable_bus_clock(SPI2_HOST, true);
    spi_ll_reset_register(SPI2_HOST);
    spi_ll_set_clk_source(&GPSPI2, SPI_CLK_SRC_XTAL);
    spi_ll_clk_source_pre_div(&GPSPI2, 1, 1);
    spi_ll_enable_clock(SPI2_HOST, true);
    spi_ll_master_init(&GPSPI2);
    GPSPI2.clock.val = 0;
    GPSPI2.clock.clk_equ_sysclk = 1; // XTAL clock; runtime selects its own clock.
    spi_ll_master_set_mode(&GPSPI2, 0);
    spi_ll_set_half_duplex(&GPSPI2, true);
    spi_ll_master_select_cs(&GPSPI2, 0);
    const uint8_t pins[] = {42, 36, 51, 35, 9, 50};
    const uint16_t signals[] = {SPI2_CK_PAD_OUT_IDX, SPI2_D_PAD_OUT_IDX,
        SPI2_Q_PAD_OUT_IDX, SPI2_WP_PAD_OUT_IDX, SPI2_HOLD_PAD_OUT_IDX,
        SPI2_CS_PAD_OUT_IDX};
    for (unsigned i=0; i<sizeof(pins); i++) {
        output(pins[i], pins[i] == 50);
        esp_rom_gpio_connect_out_signal(pins[i], signals[i], false, false);
    }

    if (!byte(0x51, 0) || !reg(0x11, NULL, 0)) return;
    esp_rom_delay_us(600000);
    if (!byte(0xfe, 0x20) || !byte(0x19, 0x10) || !byte(0x1c, 0xa0) ||
        !byte(0xfe, 0) || !byte(0xc4, 0x80) || !byte(0x36, 0) ||
        !byte(0x3a, 0x55) || !byte(0x35, 0) || !byte(0x53, 0x20) ||
        !byte(0x51, 0) || !byte(0x63, 0xff) || !paint() || !reg(0x29, NULL, 0)) return;
    esp_rom_delay_us(600000);
    if (!byte(0x51, 35u*255u/100u)) return;
    first_us = elapsed_us();
    painted = true;
}

void dg_display_handoff(void) {
    if (!painted) return;
    uint32_t words[4];
    dg_display_record(words, first_us, elapsed_us());
    REG_WRITE(STORE(1), words[1]);
    REG_WRITE(STORE(2), words[2]);
    REG_WRITE(STORE(3), words[3]);
    REG_WRITE(STORE(0), words[0]); // Last write commits this boot's frame.
}
