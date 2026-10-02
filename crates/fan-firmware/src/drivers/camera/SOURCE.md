# Camera register sources

The two unmodified vendor headers beside this file are from Espressif `esp-video-components` commit `dbbdcbd5e25bdb6561d3f7fd0c5149641ba5536f`. Rust `tables.rs` preserves their register order and values; the OV3640 list terminator is omitted and SC101IOT_YUV422_FMT_UYVY resolves to 0x02.

- https://github.com/espressif/esp-video-components/blob/dbbdcbd5e25bdb6561d3f7fd0c5149641ba5536f/esp_cam_sensor/sensors/ov3640/private_include/ov3640_dvp_8bit_24Minput_640x480_rgb565_le_7fps.h
- https://github.com/espressif/esp-video-components/blob/dbbdcbd5e25bdb6561d3f7fd0c5149641ba5536f/esp_cam_sensor/sensors/sc101iot/private_include/sc101iot_dvp_8bit_20Minput_640x480_yuv422_uyvy_15fps.h

Copyright Espressif Systems (Shanghai) CO LTD, 2026 (OV3640), 2025–2026 (SC101IOT); Apache-2.0. Headers retain their original SPDX notices. `ov3640.c`, `sc101iot.c` and sensor include headers at the same commit define ID checks, SCCB widths/page addressing, PWDN levels, stream control, and format clocks. Hardware mapping and descriptor checks follow `esp-mosaico-bsp` commit a4bfb426afcf2911c01fdd9b6d6f024019f00f8f, `mosaico_module_camera/mosaico_camera.c` and `mosaico_module_mgr/mosaico_module_mgr.c`.

OV3640: 7-bit address 0x3C, ID 0x300A/0x300B = 0x364C or 0x3641, 16-bit register/8-bit data, RGB565_LE 640×480 at 7fps, on-module 24MHz clock, PWDN low.
SC101IOT: 7-bit address 0x68, F0 page selection then 8-bit registers, ID 0x31F7/0x31F8 = 0xDA4A, UYVY 640×480 at 15fps, on-module 20MHz clock, PWDN high. The source table ends in standby; streaming is enabled via page31 register00 bit0=1, following sc101iot_set_stream().

Header SHA256:

- OV3640: `5678a681420cf401d3ef94a19beb6970ad41e3056eb17d64a4f84c3d685ee619`
- SC101IOT: `6ae7e7d1449fb5bb5b071709dd722b352f0dd1e56e9e6c40dc0c78939275e5d5`

The BSP's `components/mosaico_module_camera/patches/esp-idf-dvp-frame-capture-stability.patch` appends a 64-byte DMA tail and rejects `dma_recv_size < fb_size`; extra samples are discarded. This firmware reserves the same tail in one aligned PSRAM allocation, requires a real first VSYNC EOF in `[614400, 614464]`, stops DMA, synchronizes both descriptors and PSRAM with `number_of_received_bytes()`, and requires the synchronized receive count to cover that EOF. Only the first 614400 bytes are converted. A stopped DMA or full buffer without EOF never produces an image. Short first frames are discarded with a bounded three-attempt synchronization budget; timeout or DMA errors cancel immediately.

The descriptor flags are polled through volatile 32-bit reads in S31 internal DRAM. The locked HAL marks `soc_internal_memory_cached` false for S31 and performs cache operations only for PSRAM. No pixel memory is read while DMA owns it. The converted Slint RGB8 image owns a separate allocation.

Both GPIO33 (D2 / USJ_DM) and GPIO34 (flash / USJ_DP) reuse Serial/JTAG pads on S31. USB pad and interrupt state is saved after descriptor validation but before constructing the flash output; HAL GPIO configuration otherwise disables the pads before a later snapshot could preserve them. Cancellation restores the saved pad fields and interrupt mask. Camera data capture uses an on-module oscillator; no XCLK pin is driven.
