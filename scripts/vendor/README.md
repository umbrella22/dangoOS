# ESP-IDF partition generator

`gen_esp32part.py` and `LICENSE-ESP-IDF` come from Espressif ESP-IDF v5.5.1,
commit `fcae32885b0296b32044cb99ecbdc50d98dddb83`:

- https://github.com/espressif/esp-idf/blob/fcae32885b0296b32044cb99ecbdc50d98dddb83/components/partition_table/gen_esp32part.py
- https://github.com/espressif/esp-idf/blob/fcae32885b0296b32044cb99ecbdc50d98dddb83/LICENSE

Both files are unchanged. The generator is licensed under Apache-2.0 and retains
its source notice. Its SHA-256 is
`02f6c1c5f012393d47987ee64bc50ba319a2eda48cbcec754d8cfaf780cce529`.
The migration tool verifies that digest before generating a partition table.

The board layout is derived from the verified backup of this board. The
upstream project's separate `partitions_16MB.csv` is not used as this board's
partition map. Generation explicitly selects 16 MiB, partition offset 0x8000
and S31 bootloader offset 0x2000, then checks MD5, every parsed partition field
and the unchanged binary records for protected partitions.
