# 交接文档：现状、下一步与后续拓展

> 2026-10-23。接手时先读本文，再按「文档地图」去看细节。
> 仓库：`/Volumes/project_home/github/esp-32`，分支只在本地提交，**不推送**。

## 1. 一句话现状

ESP-Mosaico v1.2（ESP32-S31）上的 Rust 固件。主页是会对触摸、晃动、光线、磁铁、
扩展模块、电量做出反应的狐狸团子；从主页唤出应用网格，里面是原生应用（设置 / 传感器 / 计时器）
和 Lua 5.5 应用。Lua 应用可以从电脑经 USB 推送到板子上并热重载，兼容官方 `launcher.json` 格式。

N1–N7 的代码已集成在 `codex/n1-n7-integration`，每阶段留有本地评审快照，
提交范围与证据见 [执行台账](N1_N7_EXECUTION.md)。设备仍是首次 N1 镜像，
后续阶段未烧录；从 [集中真机清单](N1_N7_DEVICE_ACCEPTANCE.md) 开始验收。

## 2. 原有真机基线与已知问题

| 领域 | 内容 |
| --- | --- |
| 狐狸 | 20 个姿势 + 部件动画，352×336 画布，18–20 fps，`drop=0` |
| 情绪引擎 | 摇晃 / 狂甩 / 抚摸 / 点按阶梯、光线遮挡与强光、PIR 来访、磁铁、模块插拔、电量、睡眠与唤醒（规则见 `behavior/affect.rs` 顶部的表） |
| 传感器 | BMI270（FIFO 50 Hz）、双 BMM150、BQ27220、CST9220 触摸、交互板 LDR / PIR / 电容键 |
| 反馈 | 交互板 WS2812 心情灯（仅右槽）；操作者确认原生 Angry 有震动，先前 Lua Error 手持无震动，Lua Angry 触感未记录。待同固件、同预设对照，不能据此判定马达硬件失效 |
| 扩展槽 | EEPROM 描述符识别、左右自适应交互板；相机只识别（左槽） |
| Hub | 狐狸主页、应用网格、边缘手势、页面滑动；设置（亮度、灯光、震动）、传感器、计时器 |
| Lua L0–L2 | Lua 5.5.1、时间片调度、4 MiB / 应用内存上限；模块 `display` `delay` `system` `json` `arg_schema` `os` `io` `storage` |
| 文件系统 | `/system`（只读，官方 LittleFS 镜像，含 dino / flappybird / app_store / album_app）、`/data`（main_app 末尾 2 MiB，可写） |
| 推送 | `scripts/push-app.py`：push / run / stop / reload / ls / rm / status / reboot；CRC 校验、暂存后提交、热重载 |
| 稳定性 | 开机崩溃记录存 RTC 内存，下次启动打印并跳过出事阶段；ROM 符号冲突检查进了 `check.sh` |

内置 Lua 应用：snake、game_2048（编进固件）。`/system` 里 dino、flappybird 依赖齐全。
以上真机记录来自旧布局与早期镜像。新集成版本已补相册依赖，但原版 album_app
仍待真机验收；app_store 所需网络能力仍未实现。

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

- 旧布局的 ROM 刷机只写 main_app `0x210000`。不碰 bootloader（S31 上在 `0x2000`）
  和 `0x20000` 的出厂 `vibe_mode`。实际分区迁移必须先备份 `0x0..0x20000` 并得到明确同意；
  已准备的工具进一步要求完整 16 MiB 原始备份与恢复校验，见 [N6 迁移](N6_MIGRATION.md)。
  迁移完成后的常规 OTA 只写非运行槽及 otadata。不要在新表下刷 N1–N5/N7 历史镜像，
  其旧 data 定位会落入应用槽。工厂引导程序是否自动救援 / 回滚尚未验证，不能依赖此前猜测。
- 只在本地提交，不推送。不修改相邻仓库 `lv_binding_rust`。
- 插拔模块不屏蔽光线反应（有意保留）。

### 常用

| 做什么 | 命令 |
| --- | --- |
| 不需要板子的全部检查 | `./scripts/check.sh` |
| 固件构建 | `cargo fw --offline` |
| 旧布局刷机 / 显式 ROM 恢复（先按住 BOOT 点 RESET） | `./scripts/flash-app.sh --rom-recovery-slot0 /dev/cu.usbmodemXXXX`；不修改 otadata，不保证新布局选择 slot 0 |
| A/B 常规升级 / 两槽状态 | `python3 scripts/push-app.py ota <bin>` / `python3 scripts/push-app.py ota-status` |
| 推送应用并运行 | `python3 scripts/push-app.py push examples/lua-apps/hello_push --run --follow` |
| 板子状态 / 远程重启 | `push-app.py status` / `push-app.py reboot` |
| ROM 符号冲突 | `./scripts/check-rom-symbols.sh`（vendor 新 C 库后必跑） |

- 运行时控制台：`/dev/cu.usbmodemmosaico_s31_r11`；下载模式口：`/dev/cu.usbmodem113401`。
- `push-app.py` 需要 pyserial（esptool 自带的 Python 有）。
- 主机没连上时日志会丢，要看启动瞬间就用 `--follow`。
- 地址还原：`riscv32-esp-elf-addr2line -f -i -C -e target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw <mepc>`，必须是刚刷进去的那个 ELF。

## 5. N1–N7 实现与待验收项

### N1. 把已有驱动接成 Lua 模块（主机通过；Lua 马达输出待对照）

2026-10-22 已实现六个硬件模块及 `hardware_probe` 板级示例。`./scripts/check.sh`
通过：mosaico-s31 201、mosaico-lua 45、文件系统 10、纯驱动 42，共 298 项测试；
主机预览、固件 release 构建、627 个 C 符号的 ROM 冲突检查通过。接口、兼容差异、
源码与 ELF 指纹见 [LUA_HARDWARE.md](LUA_HARDWARE.md)。2026-10-23 已仅烧录
`main_app 0x210000`，写入校验通过；快照读数、两枚交互板键、灯带、拔插与旧句柄
失效已有真机证据。五次 Lua Error 请求进入任务但手持无震动，四次 Lua Angry 的触感
未记录。操作者随后报告原生小团子 Angry 有震动；该观察尚未与本轮启动日志和镜像指纹
绑定。N1 尚未整体验收通过，下一步先做同固件原生 / Lua / 原生 Angry 对照。

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
- 优先核对 Lua 马达输出差异：原生与 Lua 的 Angry 最终进入同一调度任务；先保持固件、
  设置和预设一致，记录原生 / Lua / 原生三段的触感及日志。不能把 `haptic: Error` 日志当成 PWM 波形或触感通过。
  另需完成触摸延迟采样、马达 150 ms 间隔的物理测量；当前没有旧固件的同场景 A/B。
  `hardware_probe` 默认 `outputs=false`；设置为 `true` 后启用灯带并在单击时请求马达。
- 默认关闭的 `haptic-diagnostics` 已构建并保存诊断 / 默认镜像，尚未烧录诊断版。
  最终默认源码再次通过 298 项主机测试、release 与 ROM 检查。固定真机探针、限时串口
  采集工具及恢复默认固件的命令已备齐；按 [N1 集中验收清单](N1_TEST_CHECKLIST.md)
  完成马达、异常 / 外部停止退灯、缺失手势和稳定性验收。诊断日志只提供运行证据，
  目前没有已证实的 Lua 触感差异根因。历史 N1 诊断镜像仅适用于旧布局；最新集成的
  诊断包、复盘核对及震动关闭取消修复见 [测试前核对](lessons/n1-n7-preflight.md)。

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

### N4. Lua 情绪引擎（代码与主机检查完成；真机待测）

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

已实现独立常驻 VM、默认 Lua Affect / Personality、原生热备、受限回调和事务热重载。
341 项主机测试、4,161 条冻结原生调用对照、12 个外部超时恶意脚本进程、21 项工具测试
及 release / ROM 检查通过。BIN 2,770,528 字节；未烧录。接口、启动时序、
资源限制、USB 命令与指纹见 [N4_LUA_BRAIN.md](N4_LUA_BRAIN.md)。

### N5. 原版 album_app（代码与主机检查完成；真机待测）

已实现 JPEG/PNG 帧、JPEG 编码、UI task 原生 LVGL 服务及独立 worker VM。
原版相册字节不变，读取 `/data/appdata/album_app/photos` 中的 JPG / JPEG / BIN；
`photo-push` 提供 CRC 校验后的原子投递。PNG 与 worker 另有独立用例。
347 项主机测试、25 项工具测试、11 个真实 LVGL 场景、48 次往返、像素对照与 worker
资源释放验收通过。BIN 2987056 字节，3 MiB 槽剩 158672 字节；未烧录。
接口、资源与兼容边界、收据见 [LUA-N5-RUNTIME.md](LUA-N5-RUNTIME.md)。

### N6. A/B 分区与 USB OTA（代码与主机检查完成；尚未迁移设备）

已实现独立于 USB 的升级核心、固件适配、命令行投递、迁移规划和完整恢复工具。
锁定的 esp-hal `bb239a0` 提供 otadata 格式与 S31 MMU 槽位识别参考；其通用 updater
受私有构造与 S31 FlashStorage 容量检测限制，未直接使用。实际读写继续通过
`esp_storage::ll` 和内部 RAM bounce，VFS / OTA 共用 FlashGuard。

| 分区 | 偏移 | 大小 | 行为 |
| --- | --- | --- | --- |
| main_app / ota_0 | `0x210000` | `0x300000` | 与旧起点和名称相同 |
| ota_1 | `0x510000` | `0x300000` | 非运行槽写入 |
| data | `0x810000` | `0x1D0000` | 新独立 LittleFS；不迁移旧数据 |
| otadata | `0x9000` | `0x2000` | 双条目、CRC、序号和状态 |

bootloader `0x2000`、vibe_mode `0x20000`、ui_apps `0x9E0000`、system `0xD00000`
保持原字节。旧布局只在 main_app 正确匹配 `0x210000 + 0x7D0000` 且无冲突分区时
使用 `0x7E0000 + 2 MiB` 的旧 data；未知表拒绝可写挂载，防止猜测分区后误格式化。

当前实现的边界：

- USB begin / chunk / end / abort / status，4096 字节顺序块与 CRC，断开后不续传。
  每块擦写、读回后确认；结束时完整读回 SHA-256，并校验 ESP 结构、应用描述符、
  XOR、内嵌 SHA-256、精确长度及 A/B 兼容标记。只在全部通过后切换元数据。
- 目标由 MMU 实际运行槽确定；与 otadata 不一致时拒绝。未确认的候选不得覆盖回退槽。
  更新暂停前台 Lua、常驻大脑和动态输出；每扇区让出执行。ACK 实际发送后才自动重启。
- 确认需要两个文件系统真实挂载、真实首帧、Lua VM 探针、连续 30 秒 UI 运行，
  大于一秒的 loop gap 重置窗口；初次空 otadata 在自检后建立 slot 0 Valid 基线。
- RTC 暖复位策略在候选第三次 main 入口选择旧槽，前两次候选启动可以失败。
  这不覆盖 main 前 / HAL 内部失败或 RTC 丢失，不代表冷断电或引导程序回滚通过。
- `ota-status` 显示两槽状态和受检 descriptor 版本；descriptor 有效不等于完整镜像通过。
  `ota-test-panic` 是默认关闭的候选故障注入，用于现场验收。
- 内置狐狸资源保留为文件系统独立兜底，当前完整镜像可容纳于 3 MiB；没有完成资源全外移。
  无线 OTA 和 HTTP 适配继续等待 Wi-Fi 支持，当前范围为 USB。

迁移工具用固定版本的官方 `gen_esp32part.py` 生成表并校验 MD5 / 原记录。
完整备份绑定设备 MAC；apply 前要求当前全 Flash 与备份一致，写后核对保护区；
restore 将旧固件、分区表、otadata 和数据逐字节还原。中断迁移需要 ROM 模式人工恢复。
实际迁移仍须现场明确同意；本轮没有设备操作。

接口、最终构建指纹和主机验收见 [N6_USB_OTA.md](N6_USB_OTA.md)，备份、plan、apply、
restore 的具体命令见 [N6_MIGRATION.md](N6_MIGRATION.md)。尚待完成：真实 USB 更新、
运行槽切换、自检 Valid、故障注入回退、十次更新保数据、冷断电边界和 vibe_mode 兼容。
这些项目统一记录在 [集中真机清单](N1_N7_DEVICE_ACCEPTANCE.md)。

### N7. 长按卸载 Lua 应用（代码与主机检查完成；真机待测）

每个前台 VM 的数据根为 `/data/appdata/<id>/`，存储于独立 VM 上下文；关闭时的
finalizer 也访问相同根。兼容默认根采用短锁复制，既有 VM 不受后续修改影响。
旧 `/data` 根文件保留；绝对路径 `io` 仍不构成沙箱。

启动器短按打开、长按确认，完整 id 与预期来源共同决定操作：Data 可卸载本体与
数据，System / Builtin 只能清数据，原生应用没有卸载动作。返回和背景可取消；
列表重排不改变目标，来源变化要求重新确认，长名称不会挤掉操作按钮。

UI 与 USB 共用管理路径，先关闭 VM 并等待 finalizer 完成，再执行文件操作。
管理状态保留到强制停止与列表刷新完成，上传 / 启动冲突明确报忙。UI 在 busy
状态帧提交并释放后才执行删除。USB 支持 `rm` 和 `rmdata`，真实完成后才回复成功，
同一 USB 包内的后续命令不会越过删除边界，原始文件内容继续按块写入。

`.trash/<id>` 与 `.clear/<id>` 记录删除 / 清数据意图；启动恢复也清理同 id 的
appdata 和未完成上传。恢复失败的 id 禁止运行或重新上传，旧数据和其他应用不动。

独立检查通过 328 项主机测试、17 项 Python 工具测试、8 个真实 LVGL 指针场景、
固件 release、预览和 ROM 符号检查。实现细节与固件指纹见
[N7_APP_LIFECYCLE.md](N7_APP_LIFECYCLE.md)。真机长按、停止时序、物理断电恢复和
提示震动仍为 not-run，集中测试见 [统一清单](N1_N7_DEVICE_ACCEPTANCE.md)。

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
| [N1_N7_EXECUTION.md](N1_N7_EXECUTION.md) | 七阶段评审分支、固定提交范围和验证台账 |
| [N1_N7_DEVICE_ACCEPTANCE.md](N1_N7_DEVICE_ACCEPTANCE.md) | 集成版本的集中真机验收顺序与记录表 |
| [N2_BUTTONS_POWER.md](N2_BUTTONS_POWER.md) | 按键、Sleep 首帧与关机 |
| [N3_SKIN_PACK.md](N3_SKIN_PACK.md) | 皮肤包、验证、原子替换与兜底 |
| [N7_APP_LIFECYCLE.md](N7_APP_LIFECYCLE.md) | 卸载、应用数据隔离与恢复 |
| [N4_LUA_BRAIN.md](N4_LUA_BRAIN.md) | 常驻大脑、行为对照与热重载 |
| [LUA-N5-RUNTIME.md](LUA-N5-RUNTIME.md) | 原版相册、图像、LVGL、worker 与独立收据 |
| [N6_USB_OTA.md](N6_USB_OTA.md) | USB OTA、自检、RTC 回退边界与收据 |
| [N6_MIGRATION.md](N6_MIGRATION.md) | 分区迁移、备份、生成计划与恢复 |
| [LUA_HARDWARE.md](LUA_HARDWARE.md) | N1 硬件接口、兼容边界、主机 / 真机证据与镜像指纹 |
| [N1_TEST_CHECKLIST.md](N1_TEST_CHECKLIST.md) | 集中真机待测顺序、诊断烧录 / 恢复命令、通过条件与记录表 |
| [MOSAICO_V12_HARDWARE.md](MOSAICO_V12_HARDWARE.md) | 引脚、电源、扩展槽 |
| [FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md) | 狐狸从 PNG 原稿到屏幕 |
| [FOX_DRAWING_GUIDE.md](FOX_DRAWING_GUIDE.md) | 角色美术怎么画 |
| [ASSET_REQUESTS.md](ASSET_REQUESTS.md) | 素材规格与待要的素材 |
| [plans/hub-shell-plan.md](plans/hub-shell-plan.md) | RFC 0006：Hub 外壳与原生应用 |
| [plans/lua-runtime-plan.md](plans/lua-runtime-plan.md) | Lua 运行时、存储布局、注册表、推送协议、实测 |
| [lessons/](lessons/README.md) | 真机调试复盘：电容触摸键、L2 开机循环与存储 |
