# 二进制资源

系统构建使用的二进制资源。完整角色来自 `dango-skin/` 独立包，安装到 NAND；
固件只内置一张狐狸团子虚影与问号，启动、加载和资源缺失时共用。

| 文件 | 用途 | 来源 | 使用者 |
| --- | --- | --- | --- |
| `fox-placeholder.png` | 352×336、8 色的系统占位图；不链接进固件 | `codex-image-gen-bridge` / `gpt-image-2.5-sunburst` 绘制后缩放、减色 | `scripts/assets/pack_placeholder.py` 的公开输入 |
| `fox-placeholder.bin` | 同一张占位图的行 RLE（5,430 字节） | `python3 scripts/assets/pack_placeholder.py`；重新生成需要 Pillow | MCU / 主机共用 `companion/render/data.rs`，无表情分图 |
| `hub-20.bin` `hub-24.bin` `hub-30.bin` | 系统页面的 Noto Sans SC MFNT 子集（4 bpp），覆盖当前 UI 文本和应用标题 | `python3 scripts/assets/gen_hub_font.py` 生成，不要手改；字体许可 SIL OFL 1.1 | `shell/theme/font.rs` |
| `hub-digits-64.bin` | 计时器大数字（Noto Sans SC Regular 64 px，只含 `0-9:`） | 同上 | `shell/theme/font.rs` |
| `bmi270-config.bin` | Bosch BMI270 配置微码（8192 B），上电后写入 IMU 才能工作 | Bosch BMI270 SensorAPI 原样拷贝，不能改 | `platform/drivers/imu.rs` |

`companion/render/data.rs` 和 `fox-placeholder.bin` 由同一脚本生成，更新时保持一致；
`--check` 检查结果是否与 PNG 一致。构建和运行无需访问图像 API 或原始大图。
完整心月狐保留 `default` ID，与其他三套团子一样通过 `Skin::parse` 从 NAND 校验加载。
system 镜像不再包含角色皮肤；已有 `default` 选择记录在补装资源后继续对应心月狐。

新增或修改 Hub 页面里的中文后，要重新运行 `gen_hub_font.py`，否则新字会显示成方框（`shell/theme/font.rs` 的测试会报缺字）。

字体构建生成带完整 SHA-256 的 MUIF 包，由 app-pack 写入 system 或导出独立字体包。
固件保留完整 20px 正文及其他字号恢复子集，完整字号集合从资源文件一次性加载到 PSRAM。
`NotoSansSC-OFL.txt` 来自 notofonts/noto-cjk 的 `Sans/LICENSE`，独立字体包随附该许可。
