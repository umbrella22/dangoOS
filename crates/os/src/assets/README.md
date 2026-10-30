# 二进制资源

系统构建使用的二进制资源。固件嵌入启动所需的资源，完整默认皮肤由主机工具打入 system。

| 文件 | 用途 | 来源 | 使用者 |
| --- | --- | --- | --- |
| `fox.bin` | 完整狐狸图像的行 RLE 调色板索引（739,512 字节） | `python3 assets/fox-dango/tools/fox_pack.py` 生成 | 主机 `app-pack`，打包为 `/system/skins/default.bin` |
| `fox-recovery.bin` | 最小 Idle 恢复画面（31,814 字节），完整皮肤不可用时显示 | 同一生成器 | MCU `fox/data.rs` |
| `hub-20.bin` `hub-24.bin` `hub-30.bin` | Hub 页面的 Noto Sans SC 子集字体（LVGL 二进制字体，4 bpp），只含 `hub/` 和 `ui/hub_view.rs` 用到的字 | `python3 scripts/assets/gen_hub_font.py` 生成，不要手改；字体许可 SIL OFL 1.1 | `ui/hub_font.rs` |
| `hub-digits-64.bin` | 计时器大数字（Noto Sans SC Regular 64 px，只含 `0-9:`） | 同上 | `ui/hub_font.rs` |
| `bmi270-config.bin` | Bosch BMI270 配置微码（8192 B），上电后写入 IMU 才能工作 | Bosch BMI270 SensorAPI 原样拷贝，不能改 | `drivers/imu.rs` |

`fox/data.rs`、`fox.bin` 和 `fox-recovery.bin` 由同一脚本一起生成，更新时保持一致。
主机校验完整皮肤，MCU 通过已有 `Skin::parse` 校验路径从只读 system 加载到 PSRAM。

新增或修改 Hub 页面里的中文后，要重新运行 `gen_hub_font.py`，否则新字会显示成方框（`ui/hub_font.rs` 的测试会报缺字）。
