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
  现有硬件模块使用只读快照与受限输出请求，官方接口及板级差异见 [LUA_HARDWARE.md](LUA_HARDWARE.md)。
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

旧分区布局的 ROM 刷机只写 `main_app`（`0x210000`）。**不要**改写 bootloader 或
`0x20000` 的 `vibe_mode`。A/B 迁移及恢复步骤见 [N6_MIGRATION.md](N6_MIGRATION.md)，
实际分区改写需要完整备份和现场确认。

```bash
# 板子进入下载模式（按住 BOOT 点一下 RESET），然后：
./scripts/flash-app.sh --rom-recovery-slot0 /dev/cu.usbmodemXXXX [镜像名]
```

脚本会构建默认固件、用 16 MiB / DIO / 40 MHz 参数生成镜像，校验完整结构和 A/B
兼容标记、打印 SHA-256，再用 esptool 写 `0x210000`。上限统一为 3 MiB。
脚本保留 otadata；在 A/B 布局下写入 slot 0 不保证下次从 slot 0 启动。
工厂救援与引导程序自动回滚仍待实测，恢复使用已核验的备份。

已迁移且本次启动自检通过的设备使用 `python3 scripts/push-app.py ota <bin>` 正常升级，
用 `ota-status` 查看两槽状态、版本和自检结果。N1–N5/N7 旧评审镜像只适用于旧布局；
它们缺少新 data 定位逻辑，不能用于迁移后的固件回退。

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

`hardware_probe` 是 N1 硬件接口的板级验收应用：

```bash
python3 scripts/push-app.py push examples/lua-apps/hardware_probe --run --follow
```

默认读取 IMU、磁力计、电池和按键快照，并在屏幕与串口显示结果；无设备或无有效样本时
显示对应错误。`launcher.json` 的 `args.pin` 默认为右槽按键 11，左槽用 13 或 12。
`args.outputs` 设为 `true` 后启用右槽六像素灯带，并在单击交互板按键时请求 `happy` 马达预设。
原版官方示例与该板级应用的验收范围见 [LUA_HARDWARE.md](LUA_HARDWARE.md)。

N1 固定接口、生命周期、预期异常、马达和限速探针位于 `examples/lua-apps/n1_*_probe/`。
`scripts/n1-device-session.py` 在既有推送协议上增加限时采集、UTC 标记和独占日志创建：

```bash
$P scripts/n1-device-session.py --log target/n1-contract-new.log \
  --wait 60 --seconds 90 --command status --push examples/lua-apps/n1_contract_probe
python3 -B scripts/tests/test_n1_device_session.py  # 离线传输模拟，无设备操作
```

退出 0 只说明传输完成；Lua 汇总、预期异常和实物效果需分别验收。
探针行为见 [N1_PROBES.md](../examples/lua-apps/N1_PROBES.md)，诊断镜像的明确烧录方式、
恢复默认固件及逐项通过条件见 [N1_TEST_CHECKLIST.md](N1_TEST_CHECKLIST.md)。
