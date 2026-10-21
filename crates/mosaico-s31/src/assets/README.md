# 二进制资源

固件用 `include_bytes!` 编进镜像的二进制文件。这里只放数据，不放代码。

| 文件 | 用途 | 来源 | 使用者 |
| --- | --- | --- | --- |
| `fox.bin` | 狐狸团子全部图像的行 RLE 调色板索引（约 644 KiB） | `python3 assets/fox-dango/tools/fox_pack.py` 生成，不要手改 | `fox/data.rs` |
| `bmi270-config.bin` | Bosch BMI270 配置微码（8192 B），上电后写入 IMU 才能工作 | Bosch BMI270 SensorAPI 原样拷贝，不能改 | `drivers/imu.rs` |

`fox/data.rs`（调色板和每张图的偏移表）和 `fox.bin` 由同一个脚本一起生成，两者必须同时提交。
