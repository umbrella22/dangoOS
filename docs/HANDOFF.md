# 交接文档：现状、下一步与后续拓展

> 2026-10-23。接手时先读本文，再按「文档地图」去看细节。
> 仓库：`/Volumes/project_home/github/esp-32`，分支只在本地提交，**不推送**。

## 1. 一句话现状

ESP-Mosaico v1.2（ESP32-S31）上的 Rust 固件。主页是会对触摸、晃动、光线、磁铁、
扩展模块、电量做出反应的狐狸团子；从主页唤出应用网格，里面是原生应用（设置 / 传感器 / 计时器）
和 Lua 5.5 应用。Lua 应用可以从电脑经 USB 推送到板子上并热重载，兼容官方 `launcher.json` 格式。

## 2. 已完成（真机验证过）

| 领域 | 内容 |
| --- | --- |
| 狐狸 | 20 个姿势 + 部件动画，352×336 画布，18–20 fps，`drop=0` |
| 情绪引擎 | 摇晃 / 狂甩 / 抚摸 / 点按阶梯、光线遮挡与强光、PIR 来访、磁铁、模块插拔、电量、睡眠与唤醒（规则见 `behavior/affect.rs` 顶部的表） |
| 传感器 | BMI270（FIFO 50 Hz）、双 BMM150、BQ27220、CST9220 触摸、交互板 LDR / PIR / 电容键 |
| 反馈 | 交互板 WS2812 心情灯（仅右槽）；马达已有 RMT PWM 驱动，但 2026-10-23 手持测试无震动，正在排查，不能沿用历史通过结论 |
| 扩展槽 | EEPROM 描述符识别、左右自适应交互板；相机只识别（左槽） |
| Hub | 狐狸主页、应用网格、边缘手势、页面滑动；设置（亮度、灯光、震动）、传感器、计时器 |
| Lua L0–L2 | Lua 5.5.1、时间片调度、4 MiB / 应用内存上限；模块 `display` `delay` `system` `json` `arg_schema` `os` `io` `storage` |
| 文件系统 | `/system`（只读，官方 LittleFS 镜像，含 dino / flappybird / app_store / album_app）、`/data`（main_app 末尾 2 MiB，可写） |
| 推送 | `scripts/push-app.py`：push / run / stop / reload / ls / rm / status / reboot；CRC 校验、暂存后提交、热重载 |
| 稳定性 | 开机崩溃记录存 RTC 内存，下次启动打印并跳过出事阶段；ROM 符号冲突检查进了 `check.sh` |

内置 Lua 应用：snake、game_2048（编进固件）。`/system` 里 dino、flappybird 依赖齐全，
app_store、album_app 缺模块（见第 5 节），在网格上变暗。

## 3. 架构速览

```text
crates/
├── mosaico-s31   库（host 可测：behavior / fox / ui / hub / telemetry）+ 固件（drivers / tasks）
├── mosaico-lua   Lua 5.5.1 + 官方 esp-claw 模块的移植层（C 用 cc 编译，host 可测）
└── mosaico-fs    littlefs 2.11.3 + 挂载表 VFS + newlib 系统调用
```

```text
传感器 / 触摸 ─► tasks::sensors / input ─► behavior::Runtime（affect + personality）
                                               │ 表情、马达、灯光
                                               ▼
       tasks::ui ── 主页：fox::FoxAnimator → FoxRenderer → canvas_pool
                 ── 原生应用：lvgl-ui → PSRAM 影子帧
                 ── Lua 应用：tasks::lua（display 画布）→ 影子帧
                                               ▼
                                   tasks::plane（中断执行器）→ QSPI DMA → CO5300
USB CDC ── console：日志输出 + 推送协议（hub::push）→ /data/apps → hub::registry
```

规则：**UI 任务独占 LVGL，plane 独占屏幕**；每个任务独占自己的硬件，任务之间只用有界队列或原子量。
不碰硬件的逻辑放库里并补 host 测试。细节见 [PROJECT_STRUCTURE.md](PROJECT_STRUCTURE.md)。

## 4. 工作流与红线

### 红线

- 刷机**只写 main_app `0x210000`**。不碰 bootloader（S31 上在 `0x2000`）、分区表（`0x8000`）
  和 `0x20000` 的出厂 `vibe_mode`。主程序起不来时，引导程序会回滚到 vibe_mode，重刷 main_app 即可。
  唯一计划内的例外是 N6 第 3 步重划分区：必须先备份 0x0–0x20000，并得到明确同意。
- 只在本地提交，不推送。不修改相邻仓库 `lv_binding_rust`。
- 插拔模块不屏蔽光线反应（有意保留）。

### 常用

| 做什么 | 命令 |
| --- | --- |
| 不需要板子的全部检查 | `./scripts/check.sh` |
| 固件构建 | `cargo fw --offline` |
| 刷机（先按住 BOOT 点 RESET） | `./scripts/flash-app.sh /dev/cu.usbmodemXXXX` |
| 推送应用并运行 | `python3 scripts/push-app.py push examples/lua-apps/hello_push --run --follow` |
| 板子状态 / 远程重启 | `push-app.py status` / `push-app.py reboot` |
| ROM 符号冲突 | `./scripts/check-rom-symbols.sh`（vendor 新 C 库后必跑） |

- 运行时控制台：`/dev/cu.usbmodemmosaico_s31_r11`；下载模式口：`/dev/cu.usbmodem113401`。
- `push-app.py` 需要 pyserial（esptool 自带的 Python 有）。
- 主机没连上时日志会丢，要看启动瞬间就用 `--follow`。
- 地址还原：`riscv32-esp-elf-addr2line -f -i -C -e target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw <mepc>`，必须是刚刷进去的那个 ELF。

## 5. 下一步（按顺序）

### N1. 把已有驱动接成 Lua 模块（主机通过；真机马达未通过）

2026-10-22 已实现六个硬件模块及 `hardware_probe` 板级示例。`./scripts/check.sh`
通过：mosaico-s31 201、mosaico-lua 45、文件系统 10、纯驱动 42，共 298 项测试；
主机预览、固件 release 构建、627 个 C 符号的 ROM 冲突检查通过。接口、兼容差异、
源码与 ELF 指纹见 [LUA_HARDWARE.md](LUA_HARDWARE.md)。2026-10-23 已仅烧录
`main_app 0x210000`，写入校验通过；快照读数、两枚交互板键、灯带、拔插与旧句柄
失效已有真机证据。马达请求虽进入任务，手持设备仍无震动，N1 尚未整体验收通过。

| Lua 模块 | 来源 | 说明 |
| --- | --- | --- |
| `imu` | `drivers/imu.rs` | BMI270 原始加速度、角速度、温度及辅助寄存器快照 |
| `magnetometer` | 同上（BMM150） | 补偿磁场、真实状态字节 + 最近报告的磁铁接近 / 接触状态 |
| `fuel_gauge` / `lib_fuel_gauge` | `drivers/battery.rs` | 电量百分比、电压、平均电流、充电状态 |
| `haptic` | `tasks/haptic.rs` | 13 种已有预设，沿用 150 ms 限速与震动设置 |
| `led_strip` | `tasks/lights.rs` | 活动交互板在右槽时可用，GPIO38、最多 6 颗 |
| `button` | 交互板电容键 | 两个活动键；带时间戳的边沿环与换板失效；AI / BOOT 见 N2 |

- Rust 侧提供只读快照与受限输出请求，C 桥接层注册模块，Lua 方法按模块放在
  `mosaico-lua/csrc/hardware/`。Lua 不碰总线；马达和灯带由原有任务执行。
- 硬件参数严格校验。上游 `arg_schema` 会取整与截断，只用于脚本参数归一化，
  不用于静默修正 GPIO、LED 下标或颜色值。
- 官方固定版本的 IMU、磁力计示例原样通过主机测试；button 原文通过 `args.pin=11`
  运行。LED 示例固定 16 颗，需适配为 6；fuel-gauge 原例依赖任意 I2C，需改成板载
  快照构造器。该上游版本没有 haptic，本模块是本项目扩展。
- `hardware_probe` 已运行 600 帧并正常结束；接口探针 14 PASS / 0 FAIL，记录到 39 个
  真实按键回调。右槽拔插后旧 LED / 按键句柄被拒绝，新句柄恢复，操作者确认红绿灯
  和恢复后的两端绿灯正常。详见 [真机记录](LUA_HARDWARE.md#2026-10-23-真机验收记录)。
- 优先排查马达实际无震动：不能把 `haptic: Error` 日志当成 PWM 波形或触感通过。
  另需完成触摸延迟采样、马达 150 ms 间隔的物理测量；当前没有旧固件的同场景 A/B。
  `hardware_probe` 默认 `outputs=false`；设置为 `true` 后启用灯带并在单击时请求马达。
- 默认关闭的 `haptic-diagnostics` 已构建并保存诊断 / 默认镜像，尚未烧录诊断版。
  最终默认源码再次通过 298 项主机测试、release 与 ROM 检查。固定真机探针、限时串口
  采集工具及恢复默认固件的命令已备齐；按 [N1 集中验收清单](N1_TEST_CHECKLIST.md)
  完成马达、异常 / 外部停止退灯、缺失手势和稳定性验收。诊断日志只提供运行证据，
  目前没有已证实的马达故障根因或修复。

### N2. AI / BOOT 按键与关机（代码与主机检查完成；真机待测）

- AI（GPIO7）、BOOT（GPIO61）低有效，还没有驱动；情绪引擎里已有 `ButtonPower` / `ButtonBoot` 事件，没人喂。
- 关机：GPIO57 开漏低有效，还没有驱动。设计「长按 AI → 狐狸 Sleep 动画 → 关机」。
- 验收：短按 / 长按 / 双击事件进情绪引擎和 Lua `button`；关机后电流归零、按键可重新开机。

实现使用独立 GPIO 采样任务、去抖与手势状态机，Lua 板载键不受交互板拔插影响。
AI 长按使系统进入终态 Sleep，只有当前 Sleep 帧成功上传并显示计时满 1500 ms 后，
才将 GPIO57 开漏拉低。306 项主机测试与固件构建通过；电源时序、实测关机电流、
光学显示和重新开机均为 not-run，见 [N2_BUTTONS_POWER.md](N2_BUTTONS_POWER.md)。

### N3. 狐狸皮肤包（代码与主机检查完成；真机待测）

- 现在狐狸的图像在 `src/assets/fox.bin`（约 740 KB）和生成的 `src/fox/data.rs`（调色板、姿势、部件偏移）里，编进固件。
- 改成数据包：`/data/skins/<id>/`，内含 `skin.json`（名字、版本、画布尺寸）+ `fox.bin`
  （把 data.rs 的表也写进 bin 头）。推送协议复用 `push-app.py`（目标目录换成 skins）。
- 固件只保留一套默认皮肤做兜底；包校验失败时退回默认皮肤并在日志里说明。
- `assets/fox-dango/tools/fox_pack.py` 增加输出皮肤包的模式；新角色按 [FOX_DRAWING_GUIDE.md](FOX_DRAWING_GUIDE.md) 画。
- 验收：推送一套换色皮肤，主页立即换装，重启后保留；删掉后回到默认。

已实现版本化包、完整元数据 / RLE 校验、可释放动态资源、缓存失效、持久选择、
暂存替换与恢复；推送命令见 [N3_SKIN_PACK.md](N3_SKIN_PACK.md)。317 项主机测试、
真实 packer → parser → renderer 检查和固件构建通过。20 姿态动态默认包与内置渲染
像素一致；当前 BIN 为 2,659,600 字节。光学切换、实物重启持久化和断电恢复仍为 not-run。
集成修复 `1b4cf33` 同时补齐 N2 实际 Sleep 姿态回执，过渡旧脸帧不再触发关机计时。

### N4. Lua 情绪引擎（狐狸的“大脑”）

- 分工：**Lua 决定做什么，Rust 负责画**。Lua 不逐像素画狐狸（352×336 × 20 fps 在 Lua 路径上做不到，单片切片最长 20 ms）。
- 新增常驻的“大脑” VM，与前台 Lua 应用并存：
  - 输入：`on_event(e)`（触摸、IMU、磁铁、模块、电量、PIR、光线、按键）和 `tick(now_ms)`；
  - 输出：`fox.set_expression`、`fox.play`、`haptic`、`led_strip`。
- 安全：
  - 原生 `affect.rs` 保留为兜底：脚本出错、超时或内存超限，自动切回原生引擎并记日志；
  - 硬实时反射留在原生（例如模块插入时的灯光反应不被抑制）；
  - 大脑脚本可以热重载，失败时保持上一版。
- 测试：把 `behavior/tests` 里的事件时间线复用为契约，在主机上同时喂给原生引擎和 Lua 大脑，比较表情序列。
- 验收：Lua 大脑复刻现有全部反应；推送新大脑后行为立即改变；故意写错脚本时狐狸照常反应。

### N5. 让 album_app 跑起来

- 缺 `lvgl`、`image`、`thread`。LVGL 已经在固件里（`lvgl-ui`、`drivers/lvgl_port.rs`），主要是 Lua 绑定。
- `image`：JPEG 已有 TJpgDec（软件）；esp-hal 不支持 S31 的硬件 JPEG。PNG 需要再 vendor 一个解码器（记得跑 ROM 符号检查）。
- `thread`：官方语义是后台 worker；可以用第二个 VM + 消息队列实现，不共享 Lua 状态。
- 验收：album_app 原样运行，浏览 `/data` 里的图片。

### N6. A/B 分区与 OTA（方案已定，未动手）

目标：板子正常运行时把新固件写进另一个槽，切换后重启；新固件起不来就自动退回旧的。
传输先走 USB，有 Wi-Fi 以后再加无线。

#### 已确认的事实

- 库已经有：锁定的 esp-hal `bb239a0` 里的 `esp-bootloader-esp-idf` 自带三个模块。
  我们的固件已经依赖它（`esp_app_desc!()`），不用换版本。
  - `partitions`：读分区表，`booted_partition()` 有 S31 分支，能读 MMU 判断当前从哪个槽启动。
  - `ota`：读写 `otadata` 和镜像状态。
  - `ota_updater::OtaUpdater`：负责下一槽查找、写入和切换。
  - 参考 `examples/ota/update/src/main.rs`。
- 库不管的部分要自己写：分区怎么划、固件从哪来、自检和回滚策略。
- 现有分区（0x8000，官方表）：

  | 分区 | 偏移 | 大小 | 说明 |
  | --- | --- | --- | --- |
  | otadata | 0x9000 | 8 KiB | 已存在 |
  | vibe_mode | 0x20000 | 1.75 MiB | app subtype 0x20（test），官方出厂固件，充当救援 |
  | main_app | 0x210000 | 0x7D0000（约 7.8 MiB） | app subtype 0x10 = ota_0；末尾 2 MiB 是 `/data` |
  | ui_apps / system | 0x9E0000 / 0xD00000 | — | 不动 |

  只有一个 OTA 槽，缺 ota_1。
- 固件实际要写入的内容约 2.6 MB（从 ELF 的 LOAD 段算的）。其中 `fox.bin` 739,512 B。

#### 计划步骤

1. 摸底，不改任何东西。
   - 用 `esptool read-flash 0x0 0x20000` 备份 bootloader、分区表和 otadata，放在仓库外。
   - 从引导程序日志和镜像里的字符串看 ESP-IDF 版本，判断有没有打开 `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`。
   - 解析 `otadata` 的两个 select 条目：seq、state、crc。
2. 先给固件瘦身（与 N3 合并）。
   - `fox.bin` 挪到 `/data/skins` 或 `/system`，固件能降到约 1.9 MB。
   - `scripts/flash-app.sh` 的上限改成槽大小。
3. 重划分区（碰红线，需要明确同意后再做）。只改 0x210000–0x9E0000 这一段：

   | 分区 | 偏移 | 大小 |
   | --- | --- | --- |
   | main_app（ota_0，名字和起点不变） | 0x210000 | 0x300000（3 MiB） |
   | ota_1 | 0x510000 | 0x300000（3 MiB） |
   | data（LittleFS `/data`） | 0x810000 | 0x1D0000（约 1.8 MiB） |

   - 用 `gen_esp32part.py` 生成二进制，表尾 MD5 由它算。只写 0x8000 这一个扇区。
   - 擦掉 `otadata`（0x9000），让引导程序回到“默认第一个槽”的状态。
   - `storage.rs` 目前从 main_app 末尾切 `/data`。改为按名字或 subtype 找独立的 data 分区，旧数据不迁移，首次启动格式化。
   - bootloader（0x2000）、vibe_mode、ui_apps、system 都不碰。
4. 固件端 `ota` 模块。
   - 新建 `crates/mosaico-s31/src/ota.rs`，或者放进 `storage` 模块旁边。
   - 写入前用 `booted_partition()` 确认目标不是正在运行的槽，不能只信 `otadata`（见“预计会遇到的问题”第 2 条）。
   - 逐扇区擦除和写入，每块 4 KiB，写入时让出 CPU，避免喂狗超时和 UI 卡死。
   - 写完从 flash 回读，算 SHA256，跟镜像尾部自带的哈希比对。
   - 校验通过后 `activate_next_partition()`，状态标为 `New`，然后重启。
5. 主机端 `push-app.py ota <bin>`。
   - 沿用现有 USB 推送协议，加 `ota_begin(size, sha256)`、`ota_chunk`、`ota_end` 三种帧。
   - 中途断开就作废，下次重新开始，不做续传。
   - 重启导致 USB 掉线属于正常（`reboot` 已经这样处理）。
6. 自检与确认。
   - 启动后做到这几项再 `set_current_ota_state(Valid)`：屏幕首帧送出、`/system` 和 `/data` 挂上、Lua VM 起来、主循环跑满 30 秒。
   - 如果引导程序不支持回滚，自己实现：在 RTC 保留内存或 `/data` 之外的固定扇区记启动尝试次数。连续 3 次没走到确认，就把 `otadata` 改回旧槽并重启。
   - 控制台加 `ota status`，打印当前槽、另一个槽的状态和版本（`esp_app_desc` 的 version 字段）。
7. 无线 OTA（等 Wi-Fi）。
   - 传输层抽象成 `trait OtaSource { fn read(&mut self, buf) }`。USB 和 HTTP 共用第 4 步的写入逻辑。

#### 预计会遇到的问题与对策

| # | 现象 | 原因 | 对策 |
| --- | --- | --- | --- |
| 1 | 改完分区表后不启动，直接进了 vibe_mode | 新表里 main_app 的起点或类型错了，或者 MD5 不对 | 进下载模式，用第 1 步的备份写回 0x8000；用 `gen_esp32part.py` 反解检查后再写 |
| 2 | OTA 把正在运行的固件写坏了 | 新镜像坏掉时引导程序会自动换槽，`otadata` 记的“当前槽”就不准了（[issue #3918](https://github.com/esp-rs/esp-hal/issues/3918)） | 目标槽一律用 `booted_partition()` 反推，两者不一致时拒绝写入并报错 |
| 3 | 切换后引导程序仍从旧槽启动 | `otadata` 的 seq 或 crc 写错，或者两个 select 条目都无效 | 回读 `otadata` 打印两个条目；必要时 `reset_data()` 再切 |
| 4 | 新固件崩溃后没有回滚 | 官方引导程序没开 `APP_ROLLBACK_ENABLE`，`New` 状态被忽略 | 第 6 步的自管启动计数；最后还有 vibe_mode 兜底，下载模式重刷 main_app 也能恢复 |
| 5 | 新固件正常，但每次重启都退回旧槽 | 开了回滚却忘了标记 `Valid` | 自检完成后必须调用；`ota status` 能看到 `PendingVerify` 卡住 |
| 6 | 写入时看门狗复位或画面卡住 | 擦大片 flash 时长时间占着临界区 | 按扇区擦写，每个扇区之间 `yield`；OTA 期间暂停灯效和 Lua 应用 |
| 7 | 写入或擦除期间崩溃 | flash 操作时 cache 被关，这时如果跑到 flash 里的代码或常量就会出错（L2 已踩过，见 `docs/lessons/l2-storage-boot.md`） | 继续用 `esp_storage::ll` 的临界区写法；回调和中断处理不要碰 flash 常量 |
| 8 | 固件超过 3 MiB，`flash-app.sh` 或 OTA 拒收 | 加 lvgl、字体、资源后变大 | 资源进文件系统（N3）；不够再把 `/data` 挪到 SPI NAND，腾出空间给槽 |
| 9 | `/data` 里推送过的应用没了 | 重划分区后 data 的位置变了，首次启动会格式化 | 重划前用 `push-app.py` 拉取备份，或者接受重新推送 |
| 10 | vibe_mode 的官方固件行为异常 | 官方固件可能按旧表找 main_app 或 `ui_apps` | 只改 main_app 的大小，名字、起点、类型都不变；改完进一次 vibe_mode 验证 |
| 11 | 主机端推送一半 USB 断了 | 线松（以前遇到过） | 不续传，重新 `ota_begin`；另一个槽没激活前旧固件不受影响 |
| 12 | 校验过了但启动报 image invalid | 镜像头里的 flash 大小或模式跟引导程序要求不一致，或者没带 app 描述符 | 用 `espflash save-image --chip esp32s31` 出 bin，参数跟 `flash-app.sh` 保持一致；`esp_app_desc!()` 不能删 |

#### 验收

- 运行 `push-app.py ota build.bin` 后自动重启进另一个槽，`ota status` 显示槽号已切换且状态为 Valid。
- 故意推送一个启动即 panic 的镜像，3 次之内回到旧槽。
- 连续 OTA 10 次，`/data` 里的应用都还在，旧槽始终可用。
- 全程不碰 0x2000 的 bootloader 和 0x20000 的 vibe_mode。

### N7. 长按卸载 Lua 应用（方案已定，未动手）

目标：在启动器里长按一个 Lua 应用的图标，确认后删除应用本体和它的数据。
Rust 原生应用（设置、传感器、计时器）不允许删除。

#### 已确认的事实

- 应用来源（`hub/registry.rs` 的 `Origin`）：

  | 来源 | 位置 | 能否卸载 |
  | --- | --- | --- |
  | Data | `/data/apps/<id>`，USB 推送的 | 能删本体和数据 |
  | System | `/system/apps/<id>`，官方只读分区 | 本体删不掉，只能清数据 |
  | Builtin | 编进固件的 Lua 应用（Snake、2048） | 本体删不掉，只能清数据 |
  | 原生 | `AppId::Settings / Sensors / Timer`，Rust | 不响应长按 |

- 同一个 id 可以同时出现在 `/data/apps` 和 `/system/apps`，`/data` 那份优先显示（遮蔽规则）。
  删掉 `/data` 那份以后，`/system` 那份会重新出现。
- USB 推送协议已经有 `rm <id>`（`hub/push.rs`）：只会 `remove_tree("/data/apps/<id>")` 然后重扫，**不删数据**。
- **应用数据目前没有隔离**。`storage.get_root_dir()` 返回 C 侧的数据根（`mosaico-lua/csrc/mosaico_platform.c`，默认 `/data`）。
  固件从没调用过 `mosaico_lua::set_data_root`，所以所有应用共用 `/data` 根目录。
  例如 `hello_push` 把计数写在 `/data/hello_push.count`，无法判断这个文件属于哪个应用。
- 文件系统已有 `Vfs::remove_tree`、`Vfs::rename`，通过 `mosaico_fs::global::with` 访问。
  `registry::scan` 会跳过以 `.` 开头的目录（`.incoming` 就是靠这个隐藏的）。
- 界面框架（`lvgl-ui`）的事件里有 `LongPressed` 和 `ShortClicked`，用 `Node::on_event` 接；还有 `components::dialog` 可以做确认框。
  LVGL 默认长按时间是 400 ms（`LV_INDEV_DEF_LONG_PRESS_TIME`）。
- 启动器图标现在用 `.on_click(AppMsg::Open(app))`（`ui/hub_view.rs`）。
  `AppMsg` 是 `Copy` 的，Lua 应用用注册表下标 `AppId::Lua(i)` 表示，重扫后下标可能变。

#### 计划步骤

1. 先给数据分家（不做这步，“删除应用数据”就无从谈起）。
   - 约定每个应用的数据目录是 `/data/appdata/<id>/`。
   - `lua::start` 之前调用 `set_data_root("/data/appdata/<id>")`，并 `create_dir_all`。
   - `storage.get_root_dir()` 就会指向自己的目录，`hello_push` 这类写法不用改。
   - 已经散落在 `/data` 根下的旧文件无法归属，留着不动，在 README 里说明。
2. 一个共享的卸载函数，放在 `hub/` 下（例如 `hub/uninstall.rs`），能在主机上测试：
   - `uninstall(vfs, id) -> Result<Outcome, Error>`，只接受来源为 Data 的应用。
   - 步骤：
     1. `rename("/data/apps/<id>", "/data/apps/.trash/<id>")`。LittleFS 的 rename 是原子的，这一步之后启动器就看不到它了。
     2. `remove_tree("/data/appdata/<id>")`，目录不存在不算错误。
     3. `remove_tree("/data/apps/.trash/<id>")`。
   - `clear_data(vfs, id)` 只做第 2 步，给 System 和 Builtin 用。
   - 启动时（`storage::init` 挂载之后）清空 `/data/apps/.trash`，处理卸载到一半断电的情况。
   - USB 的 `rm` 改成调用同一个函数，再加一个 `rmdata <id>` 命令。
3. 界面。
   - 图标从 `on_click` 改成 `on_event`：
     - `ShortClicked` 发 `Open`。
     - `LongPressed` 发 `AskUninstall(AppId)`。
     - 原生应用的图标不接 `LongPressed`。
   - `MosaicoApp` 新增 `pending_uninstall: Option<String>`，存应用 **id**，不存下标。
     收到 `AskUninstall` 时按下标查到 id 存进去，并震一下（现有 haptic 模式）。
   - 启动器页面在 `pending_uninstall` 有值时叠一个 `dialog`：
     - Data 应用：
       - 标题：卸载“<名字>”？
       - 正文：会删除应用和它保存的数据，无法恢复。
       - 如果 `/system` 有同名应用，再加一句：卸载后恢复为出厂版本。
       - 按钮：取消 / 卸载。
     - System / Builtin 应用：
       - 标题：清除“<名字>”的数据？
       - 按钮：取消 / 清除。
     - 点背景等于取消。
   - 确认后发 `ConfirmUninstall`。`update` 只负责把 id 交给执行方，不在界面回调里碰文件系统。
4. 执行与刷新。
   - `tasks/apps.rs` 的 `Action` 加 `Uninstall(String)` 和 `ClearData(String)`。
     UI 任务在主循环里取到请求后，在 `global::with` 里执行，然后 `reload` 重扫。
   - 应用较大时删除可能要几百毫秒，先在 UI 任务里做，期间对话框显示“正在卸载…”。
     实测卡顿明显再挪到单独的小任务里。
   - 完成后弹一个 toast（`components::toast`）：已卸载，或者“卸载失败：<错误>”。
5. 测试。
   - 主机测试（仿照 `push.rs` 的内存 Vfs）：
     - 卸载 Data 应用后本体和数据都没了。
     - System 和 Builtin 拒绝卸载、只能清数据。
     - 遮蔽的同名应用在卸载后重新出现。
     - 卸载到一半中断，留下的 `.trash` 在下次启动时被清掉。
     - 不存在的 id 和非法 id 返回错误。
     - `rm` 和 `rmdata` 走同一条路径。
   - 界面测试：长按不会顺带打开应用；重扫后对话框里的 id 不变。

#### 预计会遇到的问题与对策

| # | 现象 | 原因 | 对策 |
| --- | --- | --- | --- |
| 1 | 长按松手后应用被打开了 | LVGL 的 `CLICKED` 不管有没有长按都会发 | 打开改用 `ShortClicked` |
| 2 | 在滚动的应用列表里拖动时误触发长按 | 手指按住不动超过 400 ms 才算长按，但慢速拖动的开头可能被判成按住 | 收到 `ScrollBegin` 或 `PressLost` 时取消；需要的话把长按时间调到 600 ms |
| 3 | 删错了应用 | 重扫后 `AppId::Lua(i)` 指向了另一个应用 | 对话框只认 id；确认时再 `registry.find(id)` 核对一次，找不到就取消 |
| 4 | 卸载后出现一个同名的“新”应用 | 遮蔽规则，`/system` 有同名应用 | 对话框里提前说明“恢复为出厂版本”，属于正常现象 |
| 5 | 系统或内置应用也出现了“卸载” | 没区分 `Origin` | 按来源决定对话框内容；卸载函数本身也拒绝非 Data 来源，两层把关 |
| 6 | 数据没删干净 | 旧应用把文件写在 `/data` 根，或用绝对路径 `io.open` 写到了别处 | 第 1 步之后新数据都在自己的目录里；绝对路径无法拦截（Lua `io` 不是沙箱），文档写明 |
| 7 | 卸载时屏幕或灯效卡一下 | 在 UI 任务里删大目录，LittleFS 擦写占时间 | 先测；明显就挪到单独任务，或者每删一个文件让出一次 |
| 8 | 卸载到一半断电，启动器里出现残缺应用 | 删除不是原子的 | 先 rename 到 `.trash`（原子）再删；启动时清 `.trash`，`scan` 本来就跳过点目录 |
| 9 | USB 正在推送同一个应用时被卸载 | 推送暂存在 `.incoming/<id>`，commit 时会再出现 | 卸载时一并删除 `.incoming/<id>`；推送 commit 时目标已不存在属于正常 |
| 10 | 应用正在运行时被卸载 | 只有启动器页能长按，正常情况下不会；但 USB 的 `rm` 可以在应用运行时执行 | `rm` 和卸载都先判断：如果是当前应用，先 `lua::stop()` 再删 |
| 11 | 数据根在多个应用之间串了 | `set_data_root` 是全局的 C 变量，切换应用时忘了重设 | 只在 `lua::start` 一处设置；单测覆盖连续启动两个应用 |
| 12 | 对话框挡住了边缘手势，或者手势把对话框关了 | 顶部、底部、左边的手势区域归系统 | 对话框打开时返回手势等于取消；关掉对话框再执行手势 |

#### 验收

- 长按推送的 `hello_push` 后弹出卸载确认。确认后图标消失，`/data/apps/hello_push` 和 `/data/appdata/hello_push` 都不存在。
- 重新推送后计数从 1 开始。
- 长按 dino（`/system`）和 2048（内置）只出现“清除数据”；长按设置、传感器、计时器没有任何反应。
- 推送一个与 dino 同名的应用，卸载后出厂版 dino 回来了。
- 长按松手不会打开应用；快速点击仍然正常打开。
- `cargo test` 新增的卸载测试全部通过，`scripts/check.sh` 通过。

### 零碎待办

- 真机复核：电量 Tired / Dying 表情；磁力计新基线日志（`sensor mag[i] baseline ...`）。
- 换成狐狸后没重测 100 次触摸 P95 延迟。
- dino、flappybird 依赖齐全，但还没在真机上逐个玩过。
- 启动页 `SHOW_SPLASH=false`，官方点阵字体见 [ASSET_REQUESTS.md](ASSET_REQUESTS.md)。

## 6. 后续拓展

| 方向 | 现状与卡点 | 做法设想 |
| --- | --- | --- |
| 音频（ES8311，麦克风 + 喇叭） | 无驱动；esp-hal 的 S31 I2S 为 partial。引脚：I2C `0x19`，MCLK 54、BCLK 37、WS 49、DOUT 52、DIN 40、PA_EN 45 | 先 I2S 放一段 PCM 验证；再做 Lua `audio`；狐狸配音效、声音大小影响情绪 |
| 语音 / LLM | Listening / Thinking / Speaking 姿势与 `EventKind::Activity` 已就绪，没有语音来源 | 依赖音频 + 网络或 C3 协处理器，需单独立项 |
| Wi-Fi / 蓝牙 | 锁定的 esp-hal 中 S31 的 `wifi`、`bt`、`phy` 都是 `not_supported` | 等上游；之后才有 app_store（`capability`）、无线 OTA（见 N6 第 7 步）、时钟同步、天气 |
| 相机模块（左槽） | 只识别；esp-hal 相机 partial、硬件 JPEG 不支持；部分 DVP 信号与 USB Serial/JTAG 冲突 | 先抓一帧灰度验证；做 Lua `camera`；狐狸“看见”你 |
| 手柄（Handle）模块 | 只识别 | ADC 摇杆 + 按键，做 Lua `button` / `knob` |
| 红外发射 | 搁置：只有发射管（GPIO48/47，RMT NEC），没有接收头，码表未知 | 查到品牌 NEC 码表或用别的板子录码后再做；狐狸困了关灯、生气静音电视 |
| SPI NAND（SPI3） | 未使用；现在用内部 flash 的 `/data`（2 MiB） | 空间不够时把 `/data` 挪到 NAND（需要坏块管理） |
| 低功耗 | IMU 轮询；esp-hal sleep partial | IMU 中断（GPIO2）唤醒、灭屏睡眠 |
| 左槽心情灯 | GPIO15 与相机 D1 共用，暂不支持 | 插的是交互板时才启用 |
| 组件库（原 RFC 0001） | 草案，未开始，不在关键路径 | 需要时从 git 历史取回（见第 7 节） |

## 7. 已删除的历史文档

2026-10-22 清理了 `docs/legacy/`（风扇 / Slint / grok-ball 时代）、已完成或被取代的 RFC 0001–0005 及执行记录、
旧 ROADMAP 和 QUICKSTART。代码注释里还会提到 RFC 0004 / 0005，需要时从清理前的提交读：

```sh
git show 4f601de:docs/plans/grok-ball-sensor-reactions-plan.md   # RFC 0004 传感器反应
git show 4f601de:docs/plans/platform-runtime-plan.md            # RFC 0005 平台运行时
git show 4f601de:docs/plans/component-library-development-plan.md  # RFC 0001 组件库
git ls-tree -r --name-only 4f601de docs/legacy                   # 旧文档列表
```

## 8. 文档地图

| 文档 | 内容 |
| --- | --- |
| [PROJECT_STRUCTURE.md](PROJECT_STRUCTURE.md) | 源码目录、数据流、依赖方向、测试在哪 |
| [DEVELOPMENT.md](DEVELOPMENT.md) | 依赖、命令、改代码放哪、烧录、串口、推送 |
| [LUA_HARDWARE.md](LUA_HARDWARE.md) | N1 硬件接口、兼容边界、主机 / 真机证据与镜像指纹 |
| [N1_TEST_CHECKLIST.md](N1_TEST_CHECKLIST.md) | 集中真机待测顺序、诊断烧录 / 恢复命令、通过条件与记录表 |
| [MOSAICO_V12_HARDWARE.md](MOSAICO_V12_HARDWARE.md) | 引脚、电源、扩展槽 |
| [FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md) | 狐狸从 PNG 原稿到屏幕 |
| [FOX_DRAWING_GUIDE.md](FOX_DRAWING_GUIDE.md) | 角色美术怎么画 |
| [ASSET_REQUESTS.md](ASSET_REQUESTS.md) | 素材规格与待要的素材 |
| [plans/hub-shell-plan.md](plans/hub-shell-plan.md) | RFC 0006：Hub 外壳与原生应用 |
| [plans/lua-runtime-plan.md](plans/lua-runtime-plan.md) | Lua 运行时、存储布局、注册表、推送协议、实测 |
| [lessons/](lessons/README.md) | 真机调试复盘：电容触摸键、L2 开机循环与存储 |
