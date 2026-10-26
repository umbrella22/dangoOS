# 二进制资源

固件用 `include_bytes!` 编进镜像的二进制文件。这里只放数据，不放代码。

| 文件 | 用途 | 来源 | 使用者 |
| --- | --- | --- | --- |
| `fox.bin` | 狐狸团子全部图像的行 RLE 调色板索引（约 644 KiB） | `python3 assets/fox-dango/tools/fox_pack.py` 生成，不要手改 | `fox/data.rs` |
| `hub-20.bin` `hub-24.bin` `hub-30.bin` | Hub 页面的 Noto Sans SC 子集字体（LVGL 二进制字体，4 bpp），只含 `hub/` 和 `ui/hub_view.rs` 用到的字 | `python3 assets/fonts/gen_hub_font.py` 生成，不要手改；字体许可 SIL OFL 1.1 | `ui/hub_font.rs` |
| `hub-digits-64.bin` | 计时器大数字（Noto Sans SC Regular 64 px，只含 `0-9:`） | 同上 | `ui/hub_font.rs` |
| `bmi270-config.bin` | Bosch BMI270 配置微码（8192 B），上电后写入 IMU 才能工作 | Bosch BMI270 SensorAPI 原样拷贝，不能改 | `drivers/imu.rs` |

`fox/data.rs`（调色板和每张图的偏移表）和 `fox.bin` 由同一个脚本一起生成，两者必须同时提交。

新增或修改 Hub 页面里的中文后，要重新运行 `gen_hub_font.py`，否则新字会显示成方框（`ui/hub_font.rs` 的测试会报缺字）。
