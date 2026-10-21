# 执行顺序（单一事实来源）

> 2026-10-21：本文档是历史记录。`crates/grok-ball` 已移除，行为逻辑并入
> `crates/mosaico-s31/src/behavior/`，角色换成狐狸团子；当前结构见
> [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md)。

> 日期：2026-10-20。本文件只回答“先做什么、后做什么、什么可以并行”。
> 详细契约仍以各 RFC 为准；本文件不改变任何 RFC 的验收门槛。
> 目标 MVP：grok-ball 首屏 + 传感器表情 + 触觉反馈，在 S31 真机上跑通。

## 1. 一页结论

当前计划之所以显得乱，是因为把三类工作放在了同一层级：

1. **已完成的 UI 层**（RFC 0002）——不再是任务。
2. **产品内容**（RFC 0003 渲染、RFC 0004 表情）——可以 host-first 并行。
3. **平台管线**（R0/R1）——被两份 RFC 引用为硬依赖，却没有自己的计划文档。

正确的顺序是**两条泳道并行、一个汇合点**：

```text
平台泳道（风险路径）  R0 平台骨架 ──> R1 异步显示/输入 ──┐
                          └────────> R1.5 传感器任务 ────┤
                                                          ├──> C1 真机首屏 ──> C2 传感器与标定
内容泳道（host-first） G0 数据/引擎/光栅 ──> G1 自定义控件 ─┤
                        H0 表情事件引擎 ──────────────────┘
```

- **平台泳道是风险关键路径**：它是唯一有硬件未知项的工作，也是所有真机验收的前置；它不一定是最长工期，但一旦卡住就无法用 host 工作绕开。
- **内容泳道是最长工期**：G0/G1/H0 全部在 host 上完成，和 R0/R1 并行；工期长但可预测。
- **汇合点只有一个**：C1 才第一次把球、模块、马达放到同一块板上。
- **当前动作**：接板跑 R0 设备门（控制台 + 三任务心跳 + 色条）。RFC 0005、G0/G1/H0 已完成；R0 只差真机验证。

不要现在做：RFC 0001 组件库、G3 全 32 表情装饰、Hub/C3/语音/Lua。它们不阻塞 MVP。

## 2. 依赖图

```text
RFC 0002 lvgl-ui (已完成)
        │
        ├──────────────────────────────┐
        ▼                              ▼
P0.4 grok-ball 骨架                R0 平台骨架
        │                          ├─ esp-rtos + Embassy 三任务
        ├─ G0.1 数据提取            ├─ 控制台（UART0 或 USB-CDC）
        ├─ G0.2 引擎核心            ├─ 移植归档驱动
        ├─ G0.3 软件光栅            ├─ 色彩条验证
        │                          ├─> R1 异步显示/输入管线
        ├─ G1.1 CustomWidget + tick │     ├─ SpiDma<Async>
        ├─ G1.2 BallWidget          │     ├─ 双缓冲 present
        │                           │     ├─ GPIO6 IRQ + 异步 I2C
        └─ H0.1 事件契约             │     └─ P95 ≤ 50 ms
              ├─ H0.2 摇晃/触摸      └─> R1.5 传感器任务骨架
              └─ H0.3 环境/按键            ├─ shared I2C 仲裁
                    │                      ├─ BMI270 / BMM150 / 模块轮询
                    │                      └─ 有界事件通道
                    └──────────┬───────────┘
                               ▼
                      C1 真机：球 + 模块 + 马达
                               ▼
                      C2 真机：BMM150 + 性能 + 标定
                               ▼
                      Phase 4 Hub（Home/Apps、C3、PC、语音、Lua）
```

## 3. 阶段与出口门槛

| 阶段 | 名称 | 内容 | 前置 | 出口门槛 | 预估 |
| --- | --- | --- | --- | --- | --- |
| 0 | 文档与脚手架 | 本文件；RFC 0005；`crates/grok-ball` 空骨架；归档驱动盘点 | — | `cargo test -p grok-ball` 可运行；文档无互相矛盾的依赖 | 1–2 天 |
| R0 | 平台骨架 | esp-rtos + Embassy；`ui_task`/`display_task`/`input_task`；控制台；驱动移植 | 0 | 三任务无死锁；控制台可读；显示任务能刷色彩条 | 4–6 天 |
| G0 | 内容核心 | G0.1 数据、G0.2 引擎、G0.3 光栅 | 0（与 R0 并行） | `G-BALL-DATA` / `G-BALL-ENGINE` / `G-BALL-RASTER` / `G-BALL-HOST` | 9–14 天 |
| H0 | 表情引擎 | H0.1 事件契约、H0.2 摇晃/触摸、H0.3 环境/按键 | 0（与 G0 并行） | `H-EVENTS` / `H-SHAKE` / `H-TOUCH` / `H-AMBIENT` / `H-KEYS` | 10–14 天 |
| R1 | 异步管线 | 异步 SPI、双缓冲、输入任务、P95 测量 | R0 | 触摸到像素 P95 ≤ 50 ms；渲染与传输不再串行 | 5–8 天 |
| R1.5 | 传感器任务 | shared I2C 仲裁、BMI270/BMM150/模块轮询、有界事件通道 | R0（与 R1 并行） | 事件流稳定；队列满时丢最旧的非边沿事件并有计数 | 3–5 天 |
| G1 | 控件集成 | G1.1 CustomWidget + `Ui::tick`；G1.2 BallWidget + host 预览 | G0 | `G-UI-CUSTOM` / `G-S31-HOST` | 5–8 天 |
| C1 | 真机首屏 | 球上屏、右槽模块检测（H1.1）、马达（H1.3）、控制台 | R1 + R1.5 + G1 + H0 | `G-S31-DEVICE` / `H-DEVICE`；14 表情可达 | 8–13 天 |
| C2 | 传感器与标定 | BMM150（H1.2）、H2 传感器集成、性能调优、阈值标定 | C1 | `G-BALL-PERF`；摇晃/光照/PIR 阈值稳定 | 2–3 周 |
| Hub | 产品化 | Home/Apps 第二屏、C3、PC、语音、Lua | C2 | 需要新的 RFC | 另行立项 |

单人顺序执行约 **15–18 周**；平台与内容两条泳道并行时，MVP 约 **12–14 周**。
以上是各 RFC 工作包估算之和，不包含硬件返修、官方固件变更和真机调试等待。

### 阶段 0 的具体动作

1. 写 **RFC 0005：平台运行时**。把 R0/R1/R1.5 从“外部依赖”升级为有入口门槛、任务图、证据和停止条件的正式计划。
2. 建 `crates/grok-ball` 骨架（Phase 0.4），只含模块声明、许可证和空测试，作为 G0 与 H0 的共同落点。
3. 盘点 `archive/crates-pre-grok-ball-20261020/mosaico-s31` 里可复用的驱动：display、touch、shared I2C、framebuffer、input queue。R0 移植它们，不重写。
4. 处理本文档第 6 节仍标记为“待办”的两项。

## 4. 关键排序决定

### 4.1 R0/R1/R1.5 先立项，不是“以后再说”

RFC 0003 和 RFC 0004 都把 R0/R1 写成硬依赖，但仓库里没有 R0/R1 的计划、账本或门槛。这是当前计划最乱的地方，也是唯一可能卡住全部真机验收的环节。**先把 RFC 0005 写出来，再动代码。** R1.5 也归入该计划，不再作为隐性依赖。

### 4.2 内容泳道 host-first，但不提前上真机

G0/G1/H0 都可以在没有板子的情况下完成。反过来，先把球做得很好看再回头补管线，会让“视觉完成”掩盖“交互不达标”。RFC 0003 已经写明 G2 必须等 R1，这个顺序保留。

### 4.3 表达式枚举统一为 `BallExpression`，`AiState` 只是映射

RFC 0003 的引擎按 6 个 `AiState` 设计，RFC 0004 需要 14 个 `BallExpression`。如果 G0.2 按 6 个写，H0 接入时必然返工。

- 引擎输入统一为 14 值的 `BallExpression`。
- `AiState` 保留为语音/LLM 的 6 状态映射层，映射到 `Idle/Listening/Thinking/Speaking/Happy/Error`。
- G0.3 的验收截图先拍 6 个 AI 状态；H0 的 host 回放验证 14 个选择逻辑，并为新增的 8 个表情输出缩略图。

### 4.4 数据一次抽全 32 个 seed

G0.1 的提取成本几乎与数量无关，但二次提取要重做来源固定、SHA-256、许可记录和对齐测试。因此：

- 生成模块包含全部 32 个 `EMOTION_SEED` 和 25 组眼环。
- G0.3 先渲染 6 个 AI 状态做验收。
- H0 的 host 回放覆盖 14 个可达表情。
- 其余 18 个和装饰效果留在 G3，不阻塞 MVP。

### 4.5 共享 crate 只建一次

`grok-ball` 同时承载 `data`、`engine`、`raster`、`affect` 四个模块。RFC 0003 的 `-p grok-ball` 和 RFC 0004 的 `-p grok-ball` 指向同一个 crate。不要为表情引擎再建一个 crate；等它真的被第二个产品复用时再拆。

### 4.6 H1.1/H1.3 可以提前，但不必

右槽模块检测（H1.1）和马达（H1.3）只依赖 R0 的 I2C/GPIO，不依赖 R1 的异步显示。为了减少反复插拔板和切换上下文，默认放在 C1 一起做；如果想提前验证硬件，可以在 R0 之后单独做 H1.1。

### 4.7 RFC 0001 延后

RFC 0001 是可配置组件库/构建矩阵/尺寸证据，不在产品关键路径上。RFC 0002 已经交付了可用的 `lvgl-ui`，目前没有“必须靠 RFC 0001 才能继续”的阻塞。只有在以下情况才拉它：

- 字体/配置的构建不一致再次阻塞固件；
- 需要对外发布库并给出尺寸证据；
- RFC 0002 的组件缺口需要 W4 的熟悉度层。

否则放在 C2 之后，作为维护性工作。

### 4.8 遗留验证项不单独排期

页面点点击、官方 `vibe_mode` 启动、USB 串口枚举，都放到 C1 的真机窗口一起验证。它们不影响排序，但必须在 C1 的账本里留下结果。

## 5. 两条泳道的日常节奏

平台泳道有硬件未知项，内容泳道没有。建议的推进方式：

1. 先花 1–2 天写 RFC 0005 和 Phase 0.4 骨架。
2. 平台泳道进入 R0 时，内容泳道同时开始 G0.1。
3. R0 的控制台一旦可用，后续所有真机调试都走同一条日志路径。
4. R1 的 P95 测量方法在 R0 期间就定好，不要等管线写完再补测量。
5. 每次汇合前，两份 ledger 的 `clean_acceptance_count` 必须来自同一组 revision。

## 6. 文档一致性修正

以下问题会让后来者误判顺序。除最后两项外，本轮已修正：

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| RFC 0002 标题 | 仍写 `(Not Implemented)`，正文状态是 Implemented | 已改为 `(Implemented)` |
| RFC 0003 §2.3 | “The other 36 seeds” 与实际 32 个 seed 不符 | 已改为 26，并注明 RFC 0004 的 14 个可达表情 |
| RFC 0003 基线与 §2.1 | 指向未提交的 `crates/mosaico-s31` 原型 | 已改为归档路径 `archive/crates-pre-grok-ball-20261020/mosaico-s31` |
| RFC 0003 ledger | `esp32_untracked` 指向已归档 crate | 已改为 `esp32_archived` |
| 计划目录 | `lv_binding_rust/docs/plans` 与 `esp-32/docs/plans` 各有一份 | 以 `esp-32/docs/plans` 为准；RFC 0001/0002 的源码链接已改指 sibling 仓库 |
| RFC 0003/0004 ledger | 把 R0/R1 标为外部依赖，但没有文档可跳转 | 待 RFC 0005 写好后补链接 |
| RFC 0003 §7/§8.1 | 数据模块写“six expression descriptors” | 待按 §4.4 扩为 32 个描述符；需一次 RFC 修订 |

## 7. 当前进度与下一步

2026-10-20 已落地：

- RFC 0005 平台运行时计划与账本；**R0 和 R1 真机门通过**：
  - R0：USB-OTG HS CDC 控制台、心跳、整帧 480×480、黑底+三色带、触摸按下/松开。镜像 `target/mosaico-s31-r0.bin`（167,856 B，SHA-256 `6b0ca615...`）。
  - R1：异步 QSPI + 双帧 presenter、GPIO6/async I2C、LVGL `ui_task`；100 次触摸 P50 29–33ms、P95 36–41ms、max 101ms、零丢队列、76 秒无复位。镜像 `target/mosaico-s31-r1.bin`（764,448 B，SHA-256 `e362f5a9...`）。
- G0.1 数据（8 个 parity 测试）、G0.2 引擎（10 个测试）、G0.3 光栅（7 个测试、6 张 PPM、host 约 268 µs/帧）。
- H0 表情引擎（56 个时间线测试，含 `Activity/Error` 输入）。
- G1.1 `CustomWidget`/`Ui::tick`（`lvgl-ui` 全量 51 个测试）、G1.2 `BallWidget`（14 个表情的 host 预览、`creates=1`/`deletes=1`）。
- **G2.1/G2.2 真机通过**：球体 240×240、6 状态点按切换、240×240/2×1 AA/20–21fps；100 次标签触摸 P50 28–29ms、P95 30ms、max 53ms；全屏透明 clickable Obj 触摸层，点按任意位置都切换，但只有球体 240×240/8 块被重绘。
- **H1.1/H1.2 真机通过**：右槽模块 `interact(0x16)`；两颗 BMM150 在主板 I2C0 `0x11/0x12`，chip ID `0x32`，20Hz；IMU 35–46Hz；`edrops=0`。
- **H1.3 目标构建**：RMT 5kHz 马达模式表、150ms 限流、优先级；设备波形待 H2 触发。

2026-10-20 第二轮（H2 + H3 构建完成，待真机验收）：

- **H2 host 完成**：`BallRuntime`（`crates/mosaico-s31/src/runtime.rs`）在 `ui_task` 里消费 `SENSOR_EVENTS`，把 `AffectEngine` 的表情和 `PersonalityEngine` 的手势合并成一个 `BallControl` 更新；优先级 `personality > affect > resting`；两层的 `haptic` 合并后交给 `haptic::play`（150ms 限流仍由马达任务负责）。9 个 host 测试覆盖优先级、电量层、短震去重和触摸 hold-off。
- **H3 host 完成**：点按阶梯（1 下好奇 / 2 下疑惑 / 3–4 下不耐烦 / 5 下才生气）、反应延迟（单次 220ms，连击 380ms 等手势结束）、生气冷却 6s、长按 450ms 揪住并跟随手指、松手分「晕」和「惊」两种。14 个 host 测试。**真机手感必须你回来实测**：`DELAY`、`GRAB_HOLD_MS`、`TAP_WINDOW_MS` 都在 `crates/grok-ball/src/personality.rs` 顶部集中定义。
- **电量**：`drivers/battery.rs` 只读 BQ27220（0x55，I2C0，5s 轮询，不上封印/不改 CEDV），`sensor_task` 发 `BatteryLevel`；阈值 ≤15% → `Tired`、≤5% → `Dying`，迟滞 3 点。
- **启动页**：`boot_splash.rs` 手写 5x7 点阵 `ikaros`（360×120 canvas，逐字下落+下划线扫过+淡出，`TOTAL_MS` 约 1.4s），随后 `entrance.rs` 让球从屏上下落（380ms）+ 落地压扁 + 30px 回弹。触摸在整个启动+出场期间被 `ignore_touch_until` 丢弃，**不会打断**。
- **启动漏色/拖拽白线**：`clear_black()` 移到 `DISPON` 之前，`display::init` 把亮度设 0、首帧传完后才恢复；拖拽白线的根因是双缓冲滞后（第 k 帧写入的缓冲里仍是 k−2 的内容），现在记录让出的空带并存活两次渲染，同时保证 QSPI 4px 对齐窗口是 LVGL 失效区的子集。
- **可替换角色**：`skin.rs` 提供 `BLOB`/`CAT`/`FOX` 三种程序化皮肤（三角耳 + 内耳，与 16 表情完全兼容）；`sprite.rs` + `tools/sprite_pack.py` 是 WebP/GIF → RGB565 Rust 表的完整素材管线。**缺失素材清单见 [docs/ASSET_REQUESTS.md](../ASSET_REQUESTS.md)**。
- **验证镜像**：`target/mosaico-s31-h3.bin`（948,112 B，SHA-256 `ac0ea131...`），已烧到 `0x210000`。

2026-10-21 撕裂修复（待观感验收）：

- **TE = GPIO43**，厂商序列 `0x35 0x00` 已打开 V-blank 信号。`ac0ea131` 的空闲等边沿统计口径有误：`te=482` 是空闲时看到的边沿，不是真正在边沿起步的传输。
- **已烧 TE 重排版**：`target/mosaico-s31-h3-te2.bin`（948,896 B，SHA-256 `ae739a3c...`）改成“先等 job，再等边沿”，等待期间让更新的 job 抢占（`ty=`）、30ms 超时兜底；触摸帧仍直发。启动证据 `te=1282/0/386 ty=0`、`transfer_ms=12`、`fps=19`；77% 动画帧真正对齐到边沿，23% 因已有排队 job 直发。
- **真机抓到拖拽帧**：`damage=328×302`（旧+新球位并集）10 块传输 **49ms**，约 3 个面板帧；这是快速拖拽撕裂的直接来源，TE 对齐无法隐藏 3 帧的上传。P95 51ms 也被这帧拖过门槛。
- **te3 已烧**：`target/mosaico-s31-h3-te3.bin`（949,424 B，SHA-256 `37104f07...`）把 QSPI 提到 **80MHz**（S31 SPI 源时钟 160MHz 的 /2），RGB565 打包改成两像素一个 32-bit store。设备证据：典型 248×248 帧从 12–13ms 降到 **7.4ms**，`tx=11–12ms`、`te=2976/0/580`、P95 46ms/8 次触摸；80MHz 的视觉稳定性和快速拖拽样本还要你看一眼。
- **te4 已烧**：`target/mosaico-s31-h3-te4.bin`（949,728 B，SHA-256 `f4dd7d66...`）把面板初始化改成 warm path（不拉 RST、跳过 SLPOUT 和 600ms DISPON 延时，第一条命令 `0x51 0x00` 立刻压黑官方 logo），并用 `SHOW_SPLASH=false` 跳过 `ikaros` 启动页，直接进入小球出场。启动证据 `disp=1 br=1`、`fps=19`、`tx=10–12ms`。官方 logo 本身在 bootloader 里，不改 bootloader 就无法移除；这是约束内能做到的最短启动链。
- **te4 拖拽样本**：`damage=292×259` 传输 **10.1ms**、`248×248` 传输 9.1ms，已经落在 16.7ms 内，但快速拖拽仍撕裂。剩下的怀疑点是旧的“排队 job 直发”路径（启动时 `te=262/0/39`，13% 未对齐）——快速拖拽正是会排队的时刻。
- **te5 已烧**：`target/mosaico-s31-h3-te5.bin`（950,144 B，SHA-256 `9b8fb79a...`）让排队中的动画帧也等 TE，只有触摸帧直发；`te=3032/0/4`（99.9% 对齐）、`tew=7–8ms` 确认面板是 **60Hz**。
- **撕裂的真正剩余原因**：te5 的拖拽日志里多数帧 8–10ms，但偶发 **37–39ms** 帧（`damage=292×289/10 块`），同时 `raster=17ms`、IMU 读数被拉长到 25ms。这是 UI 的 ~18ms 光栅插在显示任务 chunk 之间，把 10ms 上传拉成 3 个面板帧；不是 bypass，也不是 QSPI 慢。
- **te6 已烧但性能回退**：`target/mosaico-s31-h3-te6.bin`（951,104 B，SHA-256 `031fc979...`）的握手把 `DISPLAY_ACTIVE` 覆盖到 TE 等待+传输全程，UI 跳过太多 5ms 循环，帧率掉到 **11fps**（`px` 0.68–0.74M）。结构正确（`te=240/0/0`），但代价不可接受。
- **te7 已烧**：`target/mosaico-s31-h3-te7.bin`（951,568 B，SHA-256 `92ea1de9...`）只在**实际上传的 8ms** 里压住 UI；TE 等待期间 UI 正常光栅，边沿到来时 UI 还在画就等下一个；`pending_ms` 保持 50ms 动画节拍。设备证据 `fps=19`、`te=1378/0/2`、`tew=9`，37–39ms 插队帧消失。
- **te7 还剩最后一帧**：一次大对角线甩动产生 `damage=356×349`、**17.5ms** 上传，刚好越过 16.7ms；其余帧 8.6–10.9ms。这就是用户仍能看到的残余撕裂。
- **te8 已烧**：`target/mosaico-s31-h3-te8.bin`（951,664 B，SHA-256 `89b5405d...`）给每帧位移加 72px 上限（20fps 下 1440px/s），旧+新并集最大 312×312（约 14ms）。手感用户已认可；残余撕裂来自 `damage=336×331, y0=61, 14.9ms` 这一帧：45µs/行慢于面板 34.7µs/行扫描，2.1ms 的领先在约 204 行后用完，扫描在球底追上写入——这是几何竞争，不是调度。
- **te9 已烧但被否决**：`target/mosaico-s31-h3-te9.bin`（951,792 B，SHA-256 `f8cd1fae...`）把分块行数按宽度动态化（240 宽 60 行），传输确实降到 9.9ms，但用户看到**一条把球切开的残留横线**。没有传输失败（`ok=true`），说明是 60 行大突发的 chunk 边界数据问题，不是扫描追赶；CO5300 目前只验证过 80MHz + 32 行突发。
- **te10 已回退**：32 行分块是安全工作点；te9 的 60 行突发会留下横线，不再使用。
- **官方相机为什么顺**：官方走 PPA（缩放/旋转/混合/格式转换）+ DMA，CPU 不做逐像素打包，因此大块预览能在一帧内推完；PPA 本身不消除撕裂，它消除的是 CPU 打包和调度干扰。
- **PPA 不能解决撕裂**：PPA 只做混合/缩放/旋转，不改变“什么时候写 GRAM”；撕裂是 QSPI 写入与面板扫描的时序竞争，正解是 TE + 让单次传输落在面板一帧内。

2026-10-21 定稿（用户验收）：

- **拖拽功能按产品决定删除**：personality 的 grab/drag/Struggling/Dizzy/Startled、offset/wobble/drift-home、sprite 拖拽动画槽、BallVisual/runtime 的 offset、BallWidget 的 canvas 位移/trail/4px 补边/72px 上限、ball_plane 的旧位置清带/四边擦除/trace 全部删除。
- **固定球平面**：canvas 永远在 `(120,120)-(359,359)`，每帧 8 块、约 7ms；没有“移动的方框”，方块/顶线从机制上消失。出场下落烤进位图（canvas 内 40px + 压扁 + 回弹），不再移动窗口。
- 设备验收：`p50=34 p95=42 max=42 fps=19 te=286/0/17 tew=6`，`ball_plane: chunks=8 transfer=6969us`。镜像 `target/mosaico-s31-h3-fixed4.bin`（945,920 B，SHA-256 `367b148b...`）。
- 启动 warm init + 跳过 `ikaros`：官方 bootloader logo → 黑色约 50ms → 小球出场。

下一步（grok-ball 时代的原始清单，已被下面的狐狸版清单取代）：

1. **IMU 50Hz**：用 `late/imax` 计时区分 executor 饿死与 I2C 争用，调整 sensor task 节奏。
2. **手感/表情验收**：点按阶梯、反应延迟、震动、电量表情（固定球不再有拖拽）。
3. **素材接入**：按 `docs/ASSET_REQUESTS.md` 收 WebP（拖拽素材已删除），用 `sprite_pack.py` 转码，填 `SpriteBank`。
4. **G3（可选）**：其余上游表情、彩带/zzz 等装饰，不阻塞产品。
5. **Hub/C3/语音/Lua**：Home/Apps 第二屏与 C3 控制域，需要新的 RFC。

平台管线（R0/R1）、固定球首屏、H2/H3 的 host 侧都已通过；拖拽功能已移除，显示管线定稿。


## 8. 狐狸版现状与剩余工作（2026-10-21）

已完成并真机验收：狐狸团子 20 个姿势（含 peering/dazzled）+ 部件动画；光线遮挡/强光、
拿起/持续晃/狂甩、按住/滑动抚摸三类反应（`4e558ff`）；左右槽自适应的交互模块；固定画布 19 fps。

剩余工作，按优先级：

| 优先级 | 项目 | 说明 |
| --- | --- | --- |
| 已完成 | PIR 好奇冷却 | 低电平短于 30 s 算同一次来访，只打一次招呼（`253580d`，真机确认） |
| 已完成 | IMU 稳定 50 Hz | 改为读 BMI270 FIFO，真机 50–52 Hz（传感器自身 ODR 略高）、`batch=2`、无丢帧；`imu_read` 偶尔 13 ms 是等 I2C 锁，已不影响采样 |
| P1 | 真机验收补齐 | 马达波形手感（H1.3）、电量 Tired/Dying、模块热插拔（H-DEVICE）、磁铁反应阈值 |
| P2 | BMM150 #2 读数 | `0x12` 的 \|b\| 约 1686 µT，远高于地磁，疑似靠近马达/磁性件，需确认基线与阈值 |
| P2 | 触摸延迟复测 | 换成狐狸后未重测 100 次触摸 P95 |
| P2 | 代码清理 | 未启用的全屏 presenter/display_task 回退路径 |
| P2 | 文档 | RFC 0004/0005 账本的门槛命令仍指向已删除的 `grok-ball` crate |
| P3 | 交互模块其余外设 | WS2812、SI12T、红外（RFC 0004 未开工部分） |
| P3 | 启动页 | `SHOW_SPLASH=false`；官方点阵字体见 `docs/ASSET_REQUESTS.md` |
| 另立项 | Hub / C3 / 语音 / Lua | Listening/Thinking/Speaking 姿势已就绪，但还没有语音来源驱动；需新 RFC |
| 延后 | RFC 0001 组件库 | 不在产品关键路径上 |
