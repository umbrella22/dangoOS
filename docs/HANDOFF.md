# 交接文档：现状、下一步与后续拓展

> 2026-10-26。接手时先读本文，再按「文档地图」去看细节。
> 仓库：`/Volumes/project_home/github/esp-32`，分支只在本地提交，**不推送**。

## 1. 一句话现状

ESP-Mosaico v1.2（ESP32-S31）上的 Rust 固件。主页是会对触摸、晃动、光线、磁铁、
扩展模块、电量做出反应的狐狸团子；从主页唤出应用网格，里面是原生应用（设置 / 传感器 / 计时器）
和 Lua 5.5 应用。Lua 应用可以从电脑经 USB 推送到板子上并热重载，兼容官方 `launcher.json` 格式。

实物 CoreBoard 版本已确认为 **V1.2**。官方指南页码、版本差异、引脚与电源 / USB
约束见 [V1.2 资料核对](lessons/esp-mosaico-v1.2-reference.md)。

N1–N7 的代码与现场修复已合并到本地 `main`。集成分支和七个阶段分支已清理，
历史评审提交保留在 `main` 的祖先历史中，固定提交范围与证据见
[执行台账](N1_N7_EXECUTION.md)。2026-10-26 设备已迁移至 A/B 布局，
当前普通镜像为 `0551aa22…`，出厂三列相册保留，新数据区按批准方案从空开始。
完整备份、迁移回读及 OTA 记录见下方 N6 和 [迁移记录](N6_MIGRATION.md)。
迁移前的镜像沿革见 [现场记录](N1_HAPTIC_LIVE_20261023.md#当前运行镜像)。当前继续现场验收，
最新证据见 [马达对照](N1_HAPTIC_LIVE_20261023.md)、[皮肤与 USB 现场记录](N3_SKIN_LIVE_20261024.md)、
[N4 大脑与 RESET 现场记录](N4_BRAIN_LIVE_20261024.md)
和 [集中真机清单](N1_N7_DEVICE_ACCEPTANCE.md)。N3 基本换装与清理已有证据；旧镜像有、无半包的
USB 拔线重连均无响应，RESET 后恢复。历史增强诊断通过显式单次定时软件重启完整
取回故障记录，支持TX发送阻塞后回复队列背压RX。`cb73dc6` 的只读IN端点寄存器诊断
已刷入，完整回读、引导区一致与基础通信通过。有效拔插记录显示重配后旧IN端点仍为
EPENA=1而FIFO空间充足，新写入等待旧传输结束。`c18d0ca` 重配顺序修复已完成主机与
双镜像检查，已仅刷入旧布局main_app，完整回读、引导区一致和启动查询通过。
修复后一次无半包拔插已恢复四项查询，期间无需再次RESET；重配置快照EPENA=0且有
EPDISD确认。随后半包实测自行恢复，主机检测到端口返回后约80ms收到hello、132ms收到
skin-ls；旧配色保留、完整重传自动换色及清理均通过。
`14316ff`普通`7b7b65b0…`已刷入并核对回读、引导区及启动查询；另有一轮普通版半包
真实拔插，无重启，hello63.428ms / skin-ls118.114ms恢复。同ID完整重传自动换色经
目视确认，清理ACK及skin-ls default通过，普通版清理后目视由34确认。包间yield使
上传时hb/fps持续推进，但sensor错误/丢弃/积压仍存在，整体上传实时性未通过。
客观触摸延迟、重复稳定性及普通版无半包独立用例仍待验收。

## 2. 原有真机基线与已知问题

| 领域 | 内容 |
| --- | --- |
| 狐狸 | 20 个姿势 + 部件动画，352×336 画布，18–20 fps，`drop=0` |
| 情绪引擎 | 摇晃 / 狂甩 / 抚摸 / 点按阶梯、光线遮挡与强光、PIR 来访、磁铁、模块插拔、电量、睡眠与唤醒（规则见 `behavior/affect.rs` 顶部的表） |
| 传感器 | BMI270（FIFO 50 Hz）、双 BMM150、BQ27220、CST9220 触摸、交互板 LDR / PIR / 电容键 |
| 反馈 | 交互板 WS2812 心情灯（仅右槽）；正式默认镜像 Angry / Happy / Error / Panic、震动开关与快速双击通过；最新普通镜像触摸 P95=51ms，仍未达 50ms |
| 扩展槽 | EEPROM 描述符识别、左右自适应交互板；相机只识别（左槽） |
| Hub | 狐狸主页、应用网格、边缘手势、页面滑动；设置（亮度、灯光、震动）、传感器、计时器 |
| Lua L0–L2 | Lua 5.5.1、时间片调度、4 MiB / 应用内存上限；模块 `display` `delay` `system` `json` `arg_schema` `os` `io` `storage` |
| 文件系统 | `/system`（只读，官方 LittleFS 镜像，含 dino / flappybird / app_store / album_app）、`/data`（main_app 末尾 2 MiB，可写） |
| 推送 | `scripts/push-app.py`：push / run / stop / reload / ls / rm / status / reboot；CRC 校验、暂存后提交、热重载 |
| 稳定性 | 开机崩溃记录存 RTC 内存，下次启动打印并跳过出事阶段；ROM 符号冲突检查进了 `check.sh` |

内置 Lua 应用：snake、game_2048（编进固件）。`/system` 里 dino、flappybird 依赖齐全。
以上真机记录来自旧布局与早期镜像。新集成版本已补相册依赖，原版 album_app
已完成设备源码核对、照片清点、空库显示及返回，有照片的交互仍待验收；
app_store 所需网络能力仍未实现。

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
  其旧 data 定位会落入应用槽。工厂引导程序已实测一次未确认候选 panic 后回滚，
  已确认槽冷启动及文件保留通过；未确认候选断电和 factory 救援入口仍未验证。
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

### N1. 把已有驱动接成 Lua 模块（功能部分通过；触摸延迟未达标）

六个硬件模块与 `hardware_probe` 板级示例已实现。接口、官方脚本兼容差异与阶段
主机收据见 [LUA_HARDWARE.md](LUA_HARDWARE.md)；最初无触感与后续修复的历史
保留在 [现场记录](N1_HAPTIC_LIVE_20261023.md)。当前结果如下，不把早期镜像的
主机通过推广成最终设备通过。

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

最新现场结果优先于上面的首次验收记录：正式默认镜像为 `92fbffad…`，旧布局
main_app 完整回读及引导区不变已验证，自动 watchdog 重启成功。正常 Angry/Happy/
Error/Panic 及间隔、结束停振通过；设置关闭会拒绝主页 / Lua 请求，重新开启仅提示
一次，后续新请求恢复。执行中取消、物理波形与限速仍待测。旧 100% 编码的无触感
历史保留；正式 100H/100H 修复不改变 13 种预设规格，不推断未测电气机制。

左右槽快速双击、长按均获操作者认可，正式右槽每键 3 次双击、左槽每键 2 次双击
有日志；不宣称零漏计率。左槽 GPIO13/12 可用、非活动 GPIO11/10 和右槽专用 LED API
被拒绝。换槽按 V1.2 资料先关机，再插接并用独立 POWER 键开机。
重启后约 230 秒主页采样 n=78、P50=45ms、P95=74ms、max=79ms；主观无卡顿、
触摸段 19–20 fps、drop=0，但 **P95 未达到 50 ms**。Header 警告仍存在。
`2265e33` 增加默认关闭的同样本分段诊断，主机 259 项、两种 release、714 符号和
严格镜像检查通过。诊断版实测 53 个完整样本，P95=77 ms，16 个样本跨一轮 UI；
主要耗时在 UI 出队前与跨轮等待，plane 排队中位仅 77 µs。诊断未修改门槛或调度。
后续 `cc949c9` 改为事件处理完成后重新取时推进动画，真实默认脑及像素回归通过；
组合诊断版首100样本全部同轮绘制，但严格 P95=50.756ms；随后普通镜像右槽采样
约210秒，n=65、P50=42ms、P95=51ms、max=54ms，操作者确认无卡顿、约60次点按。
50ms门槛仍未通过；完整统计与采样边界见现场记录。

三个集成修复已本地提交：`bac9e27`（去重触摸过滤）、`c6c1e95`（大脑单次编译）、
`1f97924`（100% 两段高电平）。大脑首次执行约 4.8–5.0ms，默认 Lua 脑启动通过；
全行为、热重载与故障回退留在 N4 验收。完整证据与边界见
[现场记录](N1_HAPTIC_LIVE_20261023.md) 和 [集中清单](N1_N7_DEVICE_ACCEPTANCE.md)。

### N2. AI / BOOT 按键与关机（手势、Sleep 和清灯已测；电气时序待测）

- AI（GPIO7）、BOOT（GPIO61）低有效；独立采样驱动把手势送入行为与 Lua。
- AI 长按使 GPIO57 按 Sleep 帧回执流程请求关机；重新上电使用独立 POWER 键。
- 板载键保留 180ms 连击窗口；操作者已认可，不跟随交互板改成 400ms。

实现使用独立 GPIO 采样任务、去抖与手势状态机，Lua 板载键不受交互板拔插影响。
AI 长按使系统进入终态 Sleep。当前修复要求 Sleep 帧成功上传、显示计时满 1500 ms，
并收到本次关机的黑帧发送及锁存回执后，才将 GPIO57 开漏拉低。
原阶段 306 项主机测试与固件构建通过；正式镜像已捕获两键
Lua 单击 / 双击 / 长按，AI 约 1.17–1.39 秒释放不关机、BOOT 长按不关机。Home、
Settings、Lua 三处均观察到 Sleep、屏幕关闭和 USB 断开，POWER 可重新开机；
空槽 Lua 手势也通过。右槽关机残灯问题已由 `1c793c5` 修复；组合诊断版从 Home、
Settings、Lua 三页面均有黑帧 / latch 回执，操作者确认灯灭及关机正常。物理 RESET
进入 ROM 不执行应用清理，不包含在该清灯承诺内。N2 整体验收仍有测量缺口。
精确光学时长、电流、GPIO57 波形仍待测，见 [N2_BUTTONS_POWER.md](N2_BUTTONS_POWER.md)。

### N3. 狐狸皮肤包（普通版半包恢复与重传通过；整体实时性未通过）

- 现在狐狸的图像在 `src/assets/fox.bin`（约 740 KB）和生成的 `src/fox/data.rs`（调色板、姿势、部件偏移）里，编进固件。
- 改成数据包：`/data/skins/<id>/`，内含 `skin.json`（名字、版本、画布尺寸）+ `fox.bin`
  （把 data.rs 的表也写进 bin 头）。推送协议复用 `push-app.py`（目标目录换成 skins）。
- 固件只保留一套默认皮肤做兜底；包校验失败时退回默认皮肤并在日志里说明。
- `assets/fox-dango/tools/fox_pack.py` 增加输出皮肤包的模式；新角色按 [FOX_DRAWING_GUIDE.md](FOX_DRAWING_GUIDE.md) 画。
- 验收：推送一套换色皮肤，主页立即换装，重启后保留；删掉后回到默认。

已实现版本化包、完整元数据 / RLE 校验、可释放动态资源、缓存失效、持久选择、
暂存替换与恢复；推送命令见 [N3_SKIN_PACK.md](N3_SKIN_PACK.md)。317 项主机测试、
真实 packer → parser → renderer 检查和固件构建通过。20 姿态动态默认包与内置渲染
像素一致；阶段归档 BIN 为 2,659,600 字节。后续普通集成镜像 `77f145cb…` 已完成换色推送、
同 ID 自动替换、重启保留、五类坏包拒绝、选中删除后恢复默认及重启存储确认。
旧镜像半包与无半包两次 USB 拔线重连均失败，普通 RESET 恢复。`c18d0ca` 修复的
诊断镜像已有无半包和半包拔插各一次自行恢复，半包后的完整重传、同ID自动换色与
清理通过。`14316ff`普通7b7b版一轮半包真实拔插后63.428 / 118.114ms恢复hello /
skin-ls，无重启；同ID完整重传未select自动换色，目视确认无异常；清理ACK通过，
清理后默认外观由34确认。普通baseline上传耗时24.281858秒，hb249→272、FPS15–19，
但ierr27、berr1、sensor drops166及lateness积压仍在，整体实时性未通过。客观
触摸延迟、重复稳定性、完整姿态与持续RAM趋势待验收，见 [现场记录](N3_SKIN_LIVE_20261024.md)。
集成修复 `1b4cf33` 同时补齐 N2 实际 Sleep 姿态回执，过渡旧脸帧不再触发关机计时。

### N4. Lua 情绪引擎（热加载、持久化与故障分类部分通过）

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
及 release / ROM 检查通过。阶段 BIN 2,770,528 字节；后续集成镜像默认脑已多次启动，
普通 `7b7b65b0` 已通过 Happy 热加载、语法错误候选保留旧版与一次重启保留，
默认恢复有回执。延迟运行时错误能回退原生，旧日志缺少原因，严格用例仍记 failed。
`02ce70d` 补一次性原因日志，`c86811b` 提前启动清灯；组合普通 `c8e4f00d` 已仅刷入
旧布局 main_app，完整回读、引导区一致和启动查询通过。物理 RESET 后灯灭与触摸
恢复获确认。新镜像三种延迟故障分别记录普通错误、wall 预算耗尽和内存不足，
各一次回退，后续查询与帧遥测继续；前后台调度、持久文件直接清理和默认重启已补验。
同一 effects 脑的设置禁用、Settings 预览／前台蓝灯覆盖与原生 Panic 已测；测试脑和
蓝灯页已在阶段 `33` 清理。随后发现 POWER 开机时右槽 EEPROM 首读失败后持续
`R=bad`，导致灯带未启用；`0b1b211` 已补描述符传输失败后的有限重试，普通构建和
主机检查通过。普通 `dce2f5f0…` 已仅写 main_app、完整回读及引导区核对通过，
自动启动右槽 `R=interact* derr=0`；一轮 AI 关机／POWER 开机后灯光、双键与设置
预览实测通过，首读失败重试分支尚未在设备上触发，见
[识别失败复盘](lessons/subboard-descriptor-retry.md)。
随后新目录 `06`／`07` 的自定义脑／前台蓝灯页并发关机与 POWER 恢复通过，`08`
已再次恢复默认、删除测试页及数据；`09`～`12` 又完成普通脚本错误回退后 AI 关机、
保留故障脚本时的 POWER 开机及默认恢复。首次传输失败的原因未定；全输入时间线、
数据根隔离及重复稳定性余项见
[N4 现场记录](N4_BRAIN_LIVE_20261024.md)。
`8d49214` 将内部输入改为类型化传递，
主机差分与默认 / 诊断构建通过，已随 USB 诊断镜像正常启动，普通镜像性能尚未测。接口、启动时序、
资源限制、USB 命令与指纹见 [N4_LUA_BRAIN.md](N4_LUA_BRAIN.md)。

### N5. album_app（三列出厂版与图标已交付；长期资源门槛保留）

已实现 JPEG/PNG 帧、JPEG 编码、UI task 原生 LVGL 服务及独立 worker VM。
初期使用字节不变的上游相册，读取 `/data/appdata/album_app/photos` 中的 JPG / JPEG / BIN；
`photo-push` 提供 CRC 校验后的原子投递。PNG 与 worker 另有独立用例。
347 项主机测试、25 项工具测试、11 个真实 LVGL 场景、48 次往返、像素对照与 worker
资源释放验收通过。该阶段 BIN 2987056 字节，3 MiB 槽剩 158672 字节。
后续普通集成镜像 `dce2f5f0…` 已开始真机验收：出厂相册 manifest／源码逐字节
一致、无下载版覆盖、照片与暂存目录不存在；原版空库画面及左边缘返回通过。
三张 JPEG 投递和设备大小／CRC 校验通过，颜色方向、前后切换、返回网格及删除
取消获确认；二十图实际大小／CRC 全部匹配，取消后原三图客观保留。二十图浏览
复现轻微卡顿和切图全屏黑闪，最长 slice 约 325 ms。分段实测原尺寸双线性处理
298–313 ms，读图约 13 ms。保留完整旧帧的呈现边界与图像优化已完成主机检查：
413 项 Rust／驱动、46 项 Python、原版相册等四个探针与像素对照通过。
普通修复镜像 `6143b131…` 已仅刷入旧布局 main_app，3,061,344 字节，完整回读、
引导区一致与自动启动查询通过。相同脚本三轮原尺寸处理降为 13–21 ms，80×48
缩略图降为 65–66 ms；后者仍超 20 ms 切片目标。原版二十图实屏已确认切换
不再闪黑、操作更顺畅，但帧率仍偏低。实际 81×48 缩略图三轮为 83／84／68 ms，
二十次无绘制尺寸查询为 168／169／169 ms。第二版普通 `1cbd57c0…` 已完整读回、
引导区一致并自动启动，整数缩放与有界 UI 请求窗口使相同缩略图降为 25／17／17 ms，
二十次尺寸查询为 72／78／79 ms；三十组设备输出长度／CRC 与前版一致。
操作者确认仍无错位、无黑闪，但追加的体感提升不明显，继续定位滚动／送屏路径。
后续分段诊断将网格慢帧的主要开销定位在 LVGL handler。连续点箭头意外回网格
另在主机复现为重建中的半成品对象树仍接收输入；显示事务增加 pointer 输入门控，
短按、跨提交按住及退出恢复检查通过。诊断 `939be8ae…` 已完整读回、引导区一致并
自动启动，操作者执行一轮连续切图后确认修复。实物反馈与旧采集的点按记录未同步，
不推导逐次命中率；整体滚动性能仍未验收。48 行缓冲诊断版 `8c9b37f4…` 已完整
回读并自动启动，相同网格区域末十二个保留慢帧的 handler 中位从 95.661 ms 降至
67.705 ms，flush 从 18 次降至九次；操作者体感差不多，不能据此认定硬件极限。
同一固件的只读三列临时相册沿用二十图和 81×48 缩略图，单屏相交卡片从 20 张
降为 11–15 张。操作者确认比较跟手、没有错位且边缘返回正常；保留慢帧 handler
中位进一步降至 55.319 ms，完整手势日志缺失，不换算整体 FPS。启动白屏数秒
由先构建首屏再显示的 Lua 顺序调整修复，主机像素检查与设备目视复测通过，
设备记录一次 33.619 秒打开／停止。两版临时应用均已精确清理；原版脚本保持
不变，当时二十图保留用于对照。正式交付保留 System 来源及 N7 清数据语义，
不能用同 ID 的 `/data/apps` 覆盖视为出厂更新。
截至 2026-10-26，普通 80 MHz 镜像 `0551aa22…` 已恢复并完整回读；弹窗残影保留
未解决状态。确认删除、损坏图、照片事务与正式 CLI、JPEG／BIN／PNG、17 组 worker、
相册 AI 关机／POWER 恢复、两种前台受控错误及设置／主页触摸恢复均取得对应设备证据。
十九张对照图保留，临时图片／应用已精确清理。十轮打开退出及三轮带采样控制的
追加操作未显示逐轮内部空闲下降；全局首样本的页面路径和并发状态并不一致，
差额来源尚未隔离，不能据此宣称零泄漏。长期资源、整体性能和整库清数据仍未关闭。
2026-10-26 已将 `apps/album_app` 维护版写入真实 `/system/apps/album_app`：三列网格、
首屏构建顺序修复及 96×96 相册图标。由当前设备完整备份生成 system 候选，234 个
其他目录／文件保持，完整 16 MiB 回读确认 system 以外逐字节不变，主程序仍为
`0551aa22…` 的普通 80 MHz 版本。自动启动、三份应用文件及十九张照片大小／CRC、
无下载版覆盖、默认脑／皮肤查询通过；临时核验应用已移除。操作者确认新图标、
三列十九图、无启动白屏、导航／返回，以及出厂清数据文案与取消均正常。
维护入口见 `apps/album_app/README.md`，收据位于 `target/n5-factory-album-20261026/`。
2026-10-26 已完成 N6 A/B 迁移与首次应用自检，现场验收继续，见下节。
详见 [N5 现场记录](N5_ALBUM_LIVE_20261024.md) 和集中验收表。
接口、资源与兼容边界、收据见 [LUA-N5-RUNTIME.md](LUA-N5-RUNTIME.md)。

### N6. A/B 分区与 USB OTA（设备已迁移，自检通过；OTA 验收进行中）

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
  大于一秒的 loop gap 重置窗口；应用可在自检后为仍为空的 otadata 建立 slot 0 Valid 基线。
  此设备的首次 metadata 在应用自检前已被建立，来源尚未确定。
- RTC 暖复位策略在候选第三次 main 入口选择旧槽，前两次候选启动可以失败。
  这不覆盖 main 前 / HAL 内部失败或 RTC 丢失。实物 panic 用例由工厂引导程序先行回滚，
  尚未执行到第三次 main 回退分支；已确认槽冷启动通过，未确认候选断电仍未覆盖。
- `ota-status` 显示两槽状态和受检 descriptor 版本；descriptor 有效不等于完整镜像通过。
  `ota-test-panic` 是默认关闭的候选故障注入，用于现场验收。
- 内置狐狸资源保留为文件系统独立兜底，当前完整镜像可容纳于 3 MiB；没有完成资源全外移。
  无线 OTA 和 HTTP 适配继续等待 Wi-Fi 支持，当前范围为 USB。

迁移工具用固定版本的官方 `gen_esp32part.py` 生成表并校验 MD5 / 原记录。
完整备份绑定设备 MAC；apply 前要求当前全 Flash 与备份一致，写后核对保护区；
restore 将旧固件、分区表、otadata 和数据逐字节还原。中断迁移需要 ROM 模式人工恢复。
2026-10-26 在明确授权后完成空数据区迁移。完整备份与三十个旧文件的导出在
`/Users/ikaros/mosaico-backups/20261026-n6-before-ab/`，旧四个下载应用和十九张照片
未导入新布局。写前全读匹配备份，写后全读 `e10aa029…` 与四项操作的独立预期一致，
所有其他字节保留。自动重启后新 data 挂载、默认脑／皮肤、六个出厂／内置 Lua 应用
和右槽识别正常；应用健康自检在 `hb=31` 通过，实际运行 slot 0。

首次查询发现 metadata 已为 `Valid seq=1`，同时应用健康为 `hb=5 valid=0`。
复位前 otadata 已确认擦除，状态查询代码只读，提前写入来源尚未确定。应用自检
和持久 metadata 必须分别判断；后续 PendingVerify 候选已实测拒绝自检前 begin。证据在
`target/n6-live-20261026/`，包括 `apply-01/` 和 `migration-runtime-review.json`。

接口、最终构建指纹和主机验收见 [N6_USB_OTA.md](N6_USB_OTA.md)，备份、plan、apply、
restore 的具体命令见 [N6_MIGRATION.md](N6_MIGRATION.md)。首次 OTA 的尾部回执丢失已
保留，独立查询确认新槽成功启动。主机串口读取修复通过 54 项工具测试；同固件复测
`ota-02` 在 48.605 秒收到完整 end ACK，slot 1→0、PendingVerify 保护及随后 Valid
均通过。主页 begin 静默／abort 恢复也获实物确认，默认脑已恢复。十种错误输入均被拒绝，
原 slot 0 保持 Valid；8192 B 半包的真实 USB 拔插后传输取消、通信和心跳恢复，
未执行重启，见 `10-rejection-cases/` 与 `11-partial-disconnect/root-review.json`。
十轮连续 OTA 全部通过：槽位交替、序号 4–13，每轮完整 748 块／end ACK、
PendingVerify 保护和健康确认均通过。下载应用三个文件、一张测试照片和出厂 Album
三文件的七项大小／CRC 每轮保持一致，共七十项复核；没有自动重试。单轮上传
47.383–48.394 秒，最终 slot 0 `Valid seq=13`。收据 `13-ten-ota/root-review.json`。
`14-panic-01` 激活独立故障候选后捕获 `CoreSw after panic #1` 和预定错误原因，
自动回到 slot 0 `Valid seq=13`，slot 1 为 `Aborted seq=14`。恢复自检及七项文件
核验通过。这支持工厂 bootloader 的本次回滚行为；应用 RTC 第三次回退分支被提前回滚跳过。
`15-restore-normal` 随后将故障候选替换为普通 `0551aa22…`，slot 1 达到 `Valid seq=16`，
七项文件复核通过。前台共存页仅记录 Alive 0–71 与无周期震动反馈，尚未发送本用例
begin 即暂停现场操作。默认大脑已恢复；临时页、持久化探针、独占照片与目录已清理，
出厂 Album 三文件保持原大小／CRC，见 `18b-cleanup-verification.json`。
清理后 `19-final-boot` 单次重启再次确认 slot 1 `Valid seq=16`、默认脑／皮肤、空下载
列表与右槽识别正常。最终状态在 `19-final-review.json`，串口已释放；前台升级暂停
未开始，继续该项需要重新安装临时页和测试脑。
恢复现场测试后，`24-foreground-root-review.json` 完成前台与大脑共存用例：begin 停止
前台 VM，8192 B 暂停期间实物熄灯／停振，错误 CRC 后恢复原槽、导航与重开，主页
后台输出恢复。前台旧 VM 不原地续跑，保留画面后需重新打开。默认脑和临时页清理完成。
`29-cold-root-review.json` 完成已确认槽的冷启动：关机记录 Sleep、清灯和 GPIO57 拉低，
端口消失后重新供电，启动原因为 `ChipPowerOn`；slot 1 `Valid seq=16` 与 slot 0
`Valid seq=13` 保留，本次自检通过。断电前后七项文件大小／CRC 一致；设置、出厂
相册图标及单图／返回、主页与右槽两键响应获逐项确认，日志对应导航和按键表情。
冷启动探针、单张测试照片及空目录已精确清理，出厂相册三文件保持一致；清理页随后
移除。`33-cold-final-review.json` 为当前检查点：普通 slot 1 `Valid seq=16`、默认脑／
皮肤、空下载列表和正常右槽，串口已关闭，无待执行重启。
尚待完成：未确认候选及 Flash 写入过程的断电边界、RTC 回退独立覆盖、ACK 超时、
metadata 冲突、受控迁移中断恢复和 vibe_mode 兼容。
这些项目统一记录在 [集中真机清单](N1_N7_DEVICE_ACCEPTANCE.md)。

### N7. 长按卸载 Lua 应用（主机完成；真机部分通过，临时内容已清理）

VM数据根为`/data/appdata/<id>/`；绝对路径io不构成沙箱。UI/USB管理先停止VM并
等待关闭，再操作文件与刷新列表；trash/clear持久意图提供逻辑恢复。原生无卸载，
System/Builtin仅清数据。实现、主机检查和逐项证据见[N7记录](N7_APP_LIFECYCLE.md)。

普通7b7b版截至49完成临时A/B顺序根/计数隔离、UI卸载/重装fresh、按钮/背景/Back
取消、松手不启动与单次长按触感，原生及出厂清数据文案，普通USB rm/rmdata关闭
顺序与fresh重建，真实registry重排目标稳定性，以及唯一新legacy sentinel跨B清除/
删除保留、精确移除。泛化reload可重启无关前台应用，计数按实际START解释。

49已删除两临时本体并清理sentinel，列表精确原四，skin default、brain默认CRC
bc14ae2e/error0，data465/512。B双目录有直接stat；最后A appdata删除只有管理ACK。
收据链截至`target/n7-live-20261024/partial-acceptance-through-49-supplement.json`。
Origin变化、影子覆盖、冲突、物理中断恢复、busy/toast/停页及重复稳定性未验收。
顺序数据根与单一sentinel用例不扩大为并存VM或旧/data全量审计。

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
| SPI NAND（SPI3） | 未使用；A/B 布局的内部 flash `/data` 为 1,856 KiB | 空间不够时把 `/data` 挪到 NAND（需要坏块管理） |
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
