# 开发、检查与烧录

## 依赖

- 根 `Cargo.toml` 是只有 `crates/mosaico-s31` 一个成员的 workspace。
- `esp-hal`、`esp-bootloader-esp-idf`、`esp-alloc`、`esp-rtos` 固定在 esp-hal 仓库的同一个
  Git revision（S31 支持还没进正式版）。升级时四者一起改，并做真机验证。
- `lvgl` / `lvgl-ui` 是 `../lv_binding_rust` 的 path 依赖，已测版本记在 `lvgl.rev`，
  `scripts/check-lvgl-pin.sh` 会在版本不一致或工作区有改动时提醒。
- 交叉编译工具链路径在 `.cargo/config.toml` 的 `[env]`。Cargo 不会覆盖已经设置的环境变量，
  所以换机器时在 shell 里导出自己的路径即可，不用改文件（见 README）。

## 常用命令

```bash
cargo test -p mosaico-s31 --offline                          # 库的 host 测试
./scripts/test-drivers.sh                                    # 纯驱动模块
cargo run -p mosaico-s31 --example preview --offline         # 16 表情 LVGL host 预览
cargo run -p mosaico-s31 --example splash_preview --offline  # 启动页 / 入场 PPM 帧
cargo fw --offline                                           # 固件 release 构建
./scripts/check.sh                                           # 以上全部
```

`cargo fw` 是 `.cargo/config.toml` 里的别名：
`build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf`。

## 改代码时放在哪里

- 不碰硬件的逻辑放库里（`behavior/`、`fox/`、`ui/`、`telemetry/`），并补 host 测试。
- 外设驱动放 `drivers/`；能写成纯函数的部分（报告解码、打包、几何）单独成文件，
  加进 `scripts/test-drivers.sh`。
- 新任务放 `tasks/`，在 `main.rs` 里 spawn。每个任务独占自己的硬件，任务之间只通过
  有界队列或原子量通信。
- 结构说明见 [PROJECT_STRUCTURE.md](PROJECT_STRUCTURE.md)。

## 改素材

素材管线在 `assets/fox-dango/tools/`（需要 numpy、pillow、scipy）。生成的
`src/fox/data.rs` 和 `src/fox/fox.bin` 入库，编译固件不需要 Python。流程和参数见
[FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)。

## 烧录

只写 `main_app`（`0x210000`）。**不要**改写 bootloader、分区表或 `0x20000` 的 `vibe_mode`。

```bash
# 板子进入下载模式（按住 BOOT 点一下 RESET），然后：
./scripts/flash-app.sh /dev/cu.usbmodemXXXX [镜像名]
```

脚本会构建、`espflash save-image`、打印 SHA-256，再用 esptool 写 `0x210000`。

## 串口

控制台是 USB-OTG HS CDC-ACM（不是 USB-Serial-JTAG），应用运行时枚举为
`/dev/cu.usbmodemmosaico_s31_r11`。`espflash monitor` 偶尔没有输出，这时直接用任意串口工具
（如 pyserial）打开该设备读即可。UI 任务定期打印一行统计：

```text
plane p50=<ms> p95=<ms> max=<ms> n=<样本> tx=<ms> fps=<n> anim=<µs> ras=<µs> drop=<n> fox=<表情> q=<深度>/<容量> hb=<心跳>
```

`p50/p95/max` 是触摸到首个像素的延迟，`anim`/`ras` 是上一帧动画和渲染耗时，`tx` 是上传耗时。
plane 任务在第一帧和之后每隔一段打印 `plane: frame=<n> chunks=11 transfer=<µs>us ok=<bool>`。
