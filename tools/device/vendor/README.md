# ESP-IDF 分区表生成器

`gen_esp32part.py` 与 `LICENSE-ESP-IDF` 来自 Espressif ESP-IDF v5.5.1，提交为
`fcae32885b0296b32044cb99ecbdc50d98dddb83`：

- [上游生成器](https://github.com/espressif/esp-idf/blob/fcae32885b0296b32044cb99ecbdc50d98dddb83/components/partition_table/gen_esp32part.py)
- [上游许可证](https://github.com/espressif/esp-idf/blob/fcae32885b0296b32044cb99ecbdc50d98dddb83/LICENSE)

这两个文件均保持原样。生成器采用 Apache-2.0 许可证，保留源码声明；其 SHA-256 为
`02f6c1c5f012393d47987ee64bc50ba319a2eda48cbcec754d8cfaf780cce529`。
迁移工具在生成分区表之前会核对该摘要。

板卡布局来自本机已经验证的设备备份。上游独立的 `partitions_16MB.csv` 不作为这块
板卡的分区映射。生成时显式选择 16 MiB Flash、分区表偏移 `0x8000` 和 S31
bootloader 偏移 `0x2000`，随后检查 MD5、每个解析字段，以及受保护分区的二进制
记录是否保持一致。
