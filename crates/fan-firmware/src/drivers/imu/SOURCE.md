# BMI270 configuration source

The unmodified 8192-byte `bmi270_config_file` from Bosch Sensortec's BMI270 Sensor API `bmi270.c`, header version v2.86.1 (2023-05-03).

- Source: https://github.com/boschsensortec/BMI270_SensorAPI/blob/master/bmi270.c
- Downloaded 2026-09-30, source file SHA-256: `f6446867378bbcc90ea97ef6593555de8767ca20b799355c1b37c2cabb7b7b41`
- Binary SHA-256: `2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db`
- Copyright (c) 2023 Bosch Sensortec GmbH. All rights reserved. BSD-3-Clause; full notice in `BOSCH-LICENSE`.

Initialization follows upstream `bmi2.c` `write_config_file`, `upload_file`, and `bmi2_get_internal_status`: APS disabled, INIT_CTRL=0, word-addressed INIT_ADDR (0x5B/0x5C), even-byte INIT_DATA (0x5E) chunks, INIT_CTRL=1, 20ms settle, INTERNAL_STATUS (0x21) low nibble=1. Firmware leaves APS disabled for periodic acceleration sampling. Thirty payload bytes plus a register byte fit the official BSP's 32-byte I2C write limit. ACC_CONF=0xA8 selects performance filtering, NORMAL_AVG4 and 100Hz; range=0 selects ±2g. Data is signed little-endian 16-bit at 0x0C, gated by STATUS.acc_drdy (0x03 bit7).
