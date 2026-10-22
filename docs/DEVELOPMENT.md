# 开发、检查与烧录

## 依赖

- 根 `Cargo.toml` 是 workspace，成员是 `crates/mosaico-s31`、`crates/mosaico-lua`、`crates/mosaico-fs`。
- `mosaico-lua` / `mosaico-fs` 的 C 代码用 `cc` 编译，主机测试需要 clang（`LIBCLANG_PATH`）；
  固件链接 riscv32-esp-elf 工具链里的 newlib。
- `esp-hal`、`esp-bootloader-esp-idf`、`esp-alloc`、`esp-rtos` 固定在 esp-hal 仓库的同一个
  Git revision（S31 支持还没进正式版）。升级时四者一起改，并做真机验证。
- `lvgl` / `lvgl-ui` 是 [lvgl_binding_rust](https://github.com/umbrella22/lvgl_binding_rust) 的 Git 依赖，`rev` 固定在根 `Cargo.toml`
  （`Cargo.lock` 也记着）。升级时两行一起改。想临时试本地 checkout，在自己的
  `.cargo/config.toml` 里加 `[patch."https://github.com/umbrella22/lvgl_binding_rust"]`，不要提交。
- 机器连不上 GitHub 时，第一次拉依赖用镜像配置：
  `CARGO_NET_GIT_FETCH_WITH_CLI=true GIT_CONFIG_GLOBAL=$PWD/git-proxy.config cargo fetch`，之后 `--offline` 即可。
- 交叉编译工具链路径在 `.cargo/config.toml` 的 `[env]`。Cargo 不会覆盖已经设置的环境变量，
  所以换机器时在 shell 里导出自己的路径即可，不用改文件（见 README）。

## 常用命令

```bash
cargo test -p mosaico-s31 --offline                          # 库的 host 测试
cargo test -p mosaico-lua --offline                          # Lua 运行时与模块
cargo test -p mosaico-fs --offline                           # 文件系统
./scripts/test-drivers.sh                                    # 纯驱动模块
cargo run -p mosaico-s31 --example preview --offline         # 表情 LVGL host 预览
cargo run -p mosaico-s31 --example hub_preview --offline     # Hub 各页面截图
cargo run -p mosaico-s31 --example splash_preview --offline  # 启动页 / 入场 PPM 帧
cargo fw --offline                                           # 固件 release 构建
./scripts/check-rom-symbols.sh                               # vendored C 与 S31 ROM 符号冲突
./scripts/check.sh                                           # 以上全部
```

`cargo fw` 是 `.cargo/config.toml` 里的别名：
`build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf`。

## 改代码时放在哪里

- 不碰硬件的逻辑放库里（`behavior/`、`fox/`、`hub/`、`ui/`、`telemetry/`），并补 host 测试。
- 新的 Lua 模块：C 侧放 `mosaico-lua/csrc` 或 `vendor/esp-claw`（改动记进 `vendor/NOTICE.md`），
  设备能力经 `platform.rs` 的 host 钩子注入；vendor 新的 C 库后跑 `scripts/check-rom-symbols.sh`。
- 外设驱动放 `drivers/`；能写成纯函数的部分（报告解码、打包、几何、槽位描述符、去抖）单独成文件，
  加进 `scripts/test-drivers.sh`。
- 编译期嵌入的二进制资源放 `src/assets/`，并在其 README 登记来源。
- 新任务放 `tasks/`，在 `main.rs` 里 spawn。每个任务独占自己的硬件，任务之间只通过
  有界队列或原子量通信。
- 结构说明见 [PROJECT_STRUCTURE.md](PROJECT_STRUCTURE.md)。

## 改素材

素材管线在 `assets/fox-dango/tools/`（需要 numpy、pillow、scipy）。生成的
`src/fox/data.rs` 和 `src/assets/fox.bin` 入库，编译固件不需要 Python。流程和参数见
[FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)。

## 烧录

只写 `main_app`（`0x210000`）。**不要**改写 bootloader、分区表或 `0x20000` 的 `vibe_mode`。

```bash
# 板子进入下载模式（按住 BOOT 点一下 RESET），然后：
./scripts/flash-app.sh /dev/cu.usbmodemXXXX [镜像名]
```

脚本会构建、`espflash save-image`、打印 SHA-256，再用 esptool 写 `0x210000`。
镜像超过 main_app 里 `/data` 之前的空间（6,094,848 B）时脚本拒绝烧录。
主程序起不来时引导程序会回滚到出厂的 vibe_mode，重新刷 main_app 即可。

## 串口

控制台是 USB-OTG HS CDC-ACM（不是 USB-Serial-JTAG），应用运行时枚举为
`/dev/cu.usbmodemmosaico_s31_r11`。`espflash monitor` 偶尔没有输出，这时直接用任意串口工具
（如 pyserial）打开该设备读即可。UI 任务定期打印一行统计：

```text
plane p50=<ms> p95=<ms> max=<ms> n=<样本> tx=<ms> fps=<n> anim=<µs> ras=<µs> drop=<n> fox=<表情> q=<深度>/<容量> hb=<心跳>
```

`p50/p95/max` 是触摸到首个像素的延迟，`anim`/`ras` 是上一帧动画和渲染耗时，`tx` 是上传耗时。
plane 任务在第一帧和之后每隔一段打印 `plane: frame=<n> chunks=<n> transfer=<µs>us ok=<bool>`。

## 崩溃

开机或运行中 panic 时，`crashlog.rs` 把消息、位置和所处阶段存进 RTC 快速内存，软件复位。
下次启动等 USB 连上（最多 5 s）后打印 `[boot] reset ... after panic`，并跳过上次出事的阶段
（挂载存储、扫描应用）。`mepc` 用同一个 ELF 跑 addr2line 还原，见
[lessons/l2-storage-boot.md](lessons/l2-storage-boot.md)。

## 推送 Lua 应用

```bash
P=python3   # 需要 pyserial；esptool 自带的 Python 也行
$P scripts/push-app.py push examples/lua-apps/hello_push --run --follow   # 推送、运行、跟日志
$P scripts/push-app.py ls              # 板子上的应用
$P scripts/push-app.py status          # 剩余空间
$P scripts/push-app.py run hello_push  # 也有 stop / reload / rm <id> / reboot
```

应用目录里要有 `launcher.json`（官方格式）和 `scripts/main.lua`。文件先写进
`/data/apps/.incoming/<id>`，全部 CRC 校验通过后才替换正式目录；正在运行的同名应用会自动重启。
协议细节见 [plans/lua-runtime-plan.md](plans/lua-runtime-plan.md) 第 11 节。
