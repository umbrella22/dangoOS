# 显示管线：局部更新与 RGB888

CO5300 使用 40 MHz 四线 SDR QSPI。默认固件采用 RGB565；`display-rgb888`
构建使原生 LVGL 使用 RGB888 绘制，并以 BGR888 字节存储 shadow、snapshot 和面板输入。
设置页滑块等原生控件保留 24 位颜色精度，上传只覆盖发生变化的区域。
Home、Lua framebuffer、相机及既有 RGB565 图片继续提供 RGB565 像素，在 shadow 或
驱动边界编码为所选面板格式；扩展到 RGB888 不会恢复这些来源已经丢失的颜色精度。

显示源在整个上传过程中保持不可变。LVGL 的 PARTIAL 绘制块可以在复制结束后复用，
shadow 和 snapshot 必须等到对应屏幕租约释放后才能重写。设备端仍使用发布的
`rs_lvgl / rs_lvgl_sys / rs_lvgl_ui 0.7.0`，不依赖上游尚未发布的新接口。

## 运行路径与边界

```mermaid
flowchart TD
    A[指针样本] --> B[UI 线程读取输入与 LVGL 绘制]
    B --> C[PARTIAL 回调读取实际 stride]
    C --> D[编码到面板格式 shadow 并记录多个 damage]
    D --> E[flush_ready 归还 LVGL 绘制块]
    D --> F[ScreenLease 与 PlaneJob]
    G[Home 双槽画布的 ReadLease] --> F
    H[旧页 snapshot 与新页 shadow] --> F
    F --> I{局部低延迟实验或 urgent}
    I -->|是| K[有界行分块]
    I -->|否| J[等待 TE 或超时]
    J --> K
    K --> L{连续且格式相同}
    L -->|是| M[借用来源字节上传]
    L -->|否| N[编码到 PSRAM staging 后上传]
    M --> O[每块独立 CASET / RASET / RAMWR]
    N --> O
    O --> P{完成}
    P -->|成功| Q[记录完成并释放租约]
    P -->|失败| R[保留未完成 damage 并释放租约]
    R -->|下一次 Screen job| F
```

`lvgl_port.rs` 从 `lv_display_get_buf_active(display).header.stride()` 读取当前
PARTIAL 缓冲的实际行距。`copy_to_shadow` 只编码回调包含的像素，随即调用
`lv_display_flush_ready`；原生像素指针不会保存到上传任务。
`ScreenLease::pixels` 和 `SnapshotLease::pixels` 返回的 `PixelSource` 借用租约本身。
普通 LVGL handler、主动指针读取和外部 framebuffer 写入在 shadow 被借用时暂停。
`DisplayPort` 的析构要求所有租约和原生对象先释放。

`PixelSource` 区分数值 RGB565、RGB565 LE、RGB565 BE 和 BGR888，显式携带尺寸与
字节行距。RGB565 的位扩展复制高位，反向量化保持原始 16 位值。BGR888 到 BGR888
传递完整的三个颜色字节。现有 EmbedScope 最终画面捕获协议仍输出 RGB565，
`embedscope/frame.rs` 在诊断边界执行量化，诊断截图不能证明 RGB888 的低位颜色精度。

## Damage、页面恢复与输入

`Damage` 保存最多 16 个 inclusive 矩形。扩大到驱动的四列、两行窗口，
额外像素取自已经初始化的 shadow。CO5300 V0.00 §7.5.21（p159）及
§7.5.22（p161）要求列、行起点和范围均为偶数；四列沿用本板原有的更严格约束。
仅当包围矩形的像素量不大于分别发送时合并；
矩形数量达到上限时回退为覆盖全部变化的包围矩形。

`PlaneUi::submit` 合并被替换 Screen job 的 damage。上传失败时 plane 累积
`pending_screen`；下一次 Screen 提交覆盖这些区域。完成 Home 或 Slide 上传后，
`pending_screen` 标为全屏，使缓存的普通页面能够恢复完整内容。
失败本身不生成新帧，重试仍需要后续 job。SPI 超时沿用驱动的 `faulted` 状态，
后续上传拒绝执行，直到硬件重新初始化；普通可重试错误会在当前矩形上重试一次。

Home 的每个双槽画布保存完整的 480×480 RGB565 视口。`FoxRenderer::render_panel`
在视口内放置原尺寸角色，`status::render_panel` 在发布前合入固定位置的连接图标；
`ReadLease` 保护同一张完整帧直到传输结束。plane 从面板原点按整宽行块发送完整
Home，启动、页面返回、上传失败后的下一帧和唤醒均覆盖全部旧像素。失败帧不确认
亮度恢复。独立状态带上传和 `HomeRows` 已移除，避免主体上传先擦黑连接图标。
角色源图仍为 352×336，皮肤坐标与面板视口坐标分别由渲染器和显示源描述。
普通 LVGL 页继续按 damage 局部上传。

指针样本设置 pending 标记，UI 线程在 shadow 空闲时主动调用 LVGL indev 读取，
无需等待下一次 indev timer。两轮之间完成的短按通过 latch 先报告 press，下一轮
补发 release。原生刷新与 indev timer 周期均为 16 ms；这不等于设备达到 60 FPS。
静态页无需提交像素，输入处理继续按 UI 循环执行。

## 上传、面板配置与内存

`board-s31/src/pixels.rs::RowChunks` 按整行对分块，单块上限 32,736 B。
源区域可以从奇数行开始，但高度必须为偶数；驱动另行验证实际目的行起点与高度。
480×480 的完整 Home 或普通全屏 RGB565 / RGB888 分别需要 15 / 22 块，
像素字节数为 460,800 / 691,200 B。合并前的 352×336 Home 使用 46 / 30 行分块，
原 31 行 RGB888 块违反独立 RASET 窗口的两行要求；当前路径同样强制偶数行窗口。
`PixelRows` 为 push 提供不可变的逐行编码源。push 每帧由
`transition::rows` 把旧页与新页的对应列段拼成整行，按整宽行块自上而下发送，
窗口数与全屏相同，写入顺序也与全屏一致；这条路径总是经 staging 打包。
新页只绘制一次，每个动画位置仍需传完整屏幕。
当前实现不额外分配 push 合成帧。
转场准备先保存旧 shadow，再完成当前页面尚未执行的 reconcile 并绘制新页。
动画的 280 ms 时钟在新页绘制结束后启动，准备页面的耗时不占用转场时长。
这也覆盖常规消息处理之后才到达的诊断切页和团子脚本导航，避免下一轮才准备新页，
导致第一帧后直接跳到接近终点。

连续且格式匹配的来源字节直接传给锁定 HAL 的 `half_duplex_write_async`。
该 HAL 对可访问的 DRAM/PSRAM 使用 InPlace TX，负责发送前 cache writeback，
future 的借用持续到 DMA 完成。窄区域或格式不同的来源先编码到驱动独占的
两块各 32,736 B、128 B 对齐的 PSRAM staging。第一块准备完成后，先轮询本块
DMA future 使其开始传输，再在另一块准备下一批像素；两个 future 完成后才交换
缓冲角色。直接来源仍借用原始像素，不会为轮换而额外拷贝。短于 32 B 的异步命令
使用 HAL FIFO 路径。
每个像素块独立设置 CASET/RASET 并使用 RAMWR，未采用依赖连续写游标的 RAMWRC。

阻塞启动写入仍经 HAL 内部 TX 缓冲。TX 缓冲为 4,096 B，因此冷启动清黑按容量
选择 RGB565 四行或 RGB888 两行的小带，不能直接套用异步 DMA 分块尺寸。
默认 RGB565 配置为 `COLMOD=0x55, MADCTL=0x00`；RGB888 为
`COLMOD=0x77, MADCTL=0x08`，匹配 BGR888 字节顺序。
冷启动、RGB565 bootloader handoff 接管和 sleep-out 均应用运行时格式；冷启动与
sleep-out 都在 SLPOUT 稳定后写 MADCTL/COLMOD，与 bootloader 顺序一致。
bootloader handoff 协议仍为 RGB565。已有 GRAM 内容、颜色顺序和唤醒效果需要面板验收。

原内部 staging 30,720 B 和 TX 30,720 B 共占 61,440 B；当前 TX 4,096 B，
staging 移到 PSRAM，相关内部缓冲减少 57,344 B（56 KiB）。双核构建的 RGB888
24 行 draw buffer 比 RGB565 多 11,520 B；shadow 与 snapshot 合计增加
460,800 B（450 KiB）PSRAM，另分配 65,472 B PSRAM staging 和普通堆中的
5,440 B Home 状态缓存。
堆元数据、线程栈、最大连续块和无线/音频争用后的实际余量以设备记录为准。

`display-low-latency` 是单独的 A/B 实验：仅 Screen damage 像素总量不超过半屏时
跳过 TE。整页、push 和 Home 继续等待 TE，urgent 沿用既有旁路。
阈值仅控制等待策略，不使较大的 damage 回退成整屏。
`display-metrics` 在每个 job 记录 `display.frame`：完成状态、格式、总耗时、
TE 等待、提交字节、直接来源字节、窗口数、编码时间和传输时间。
`pack_us` 是实际 CPU 打包时间之和；`transfer_us` 包括窗口命令、DMA 等待和
调度，轮换后也覆盖与本次 DMA 重叠的下一块打包，不能再将两者相加作为串行总耗时。
`bytes`/`windows` 记录尝试提交的工作量，失败或重试不能当作成功到达面板的净像素量。

## 主机复现与证据

```sh
# RGB888：生产页面、真实 LVGL 指针与回调、内存面板
cargo sim-display --locked --offline -- target/display-rgb888-20261112/simulator-rgb888
# 相同操作的 RGB565 对照
cargo run -p os --example display_pipeline --features display-simulator --locked --offline -- target/display-rgb888-20261112/simulator-rgb565
# Pillow 导出自包含播放器、PNG 与 GIF
python3 scripts/assets/export_display_replay.py \
  --rgb888 target/display-rgb888-20261112/simulator-rgb888/replay.json \
  --rgb565 target/display-rgb888-20261112/simulator-rgb565/replay.json \
  --out target/display-rgb888-20261112/review
```

`display_pipeline` 回放设置音量、设置亮度与返回启动器的 push。
每个局部上传后逐字节比较内存面板与完整 shadow；每个 push 中间位置另用
不依赖 `transition::bands` 的逐像素映射核对旧、新页位置。
两个设置页均等待松手动画结束，再执行 10 轮空闲循环，要求零像素提交。
播放器的展示速度由 HTML/GIF 指定，不能作为上传帧率或实际触控延迟。

首次性能对照前的主机验证结果（历史源码见表后说明）：

| 检查 | 结果 |
| --- | --- |
| 默认 RGB565 board / OS 库 | 20 / 386 通过；OS 2 项 ignored |
| RGB888 + simulator board / OS 库 | 20 / 388 通过；OS 2 项 ignored |
| 完整 workspace（含 doctest） | 587 通过、2 ignored、0 失败 |
| 原生 UI 可靠性集 | 4 通过 |
| Python 主机脚本 | 130 通过，含 2 项设备日志分析回归 |
| 独立驱动集 | 首轮实现阶段 103 通过 |
| RGB565 / RGB888 回放 | 各 107 帧、107 次完整画面对照通过 |
| RGB565 / RGB888 / RGB888 + 局部 TE 旁路 / 默认无诊断固件 | `d02aabd` 四种构建和资源预算通过 |
| RGB888 examples、EmbedScope、TS、recovery | 前期构建通过 |

完整 workspace、RGB888 OS、board、原生 UI 与 Python 的最新主机测试对应
`f0136e5`，原始日志及 SHA256 见
`target/display-device-20261112/host-validation.json`。随后 `d02aabd` 只修改设备端
转场准备流程，四种固件重新构建并上机复核；它不进入主机回放路径。

本轮修改的 Rust 文件通过 rustfmt，diff 无空白错误。全仓 `cargo fmt --all -- --check`
仍报告先前提交中 6 个 companion/preview 文件的格式差异，详情保存在
`workspace-rustfmt.log`；这些文件没有纳入本轮显示改动。

主机回放得到的音量、亮度拖动提交量为 RGB888 50,328–60,504 B，
RGB565 33,552–40,336 B。40 MHz quad SDR 的纯像素数据时间分别为
2.516–3.025 ms 和 1.678–2.017 ms。全屏 push 分别为 691,200 / 460,800 B，
纯数据时间为 34.56 / 23.04 ms。以上时间均为字节量计算，未计命令、DMA、
TE、调度、PSRAM 争用和屏幕光学响应。

本地证据位于 `target/display-rgb888-20261112/`：`validation.json` 保存构建/测试
命令与日志指纹，`simulator-validation.json` 保存回放命令，两个
`simulator-*/replay.json` 保存逐帧区域与数据量；`review/index.html` 包含完整 PNG
回放。固件 ELF 按配置另存，避免后一次构建覆盖前一次候选。
这些主机证据未覆盖真实 DMA 完成、面板扫描、设备内存余量和光学效果。

## 设备对照入口

`display-metrics` 同时提供 `display-test` 控制台实验入口：`status` 返回启动时
17 次连续 GPIO TE 边沿观察的周期摘要；`page home|launcher|settings` 在常规 UI
消息处理前变更页面，经同一轮 reconcile 和页面生命周期后再渲染；
`pattern colors|gradient|off` 由 UI 线程管理原生 LVGL 测试图；
`swipe x0 y0 x1 y1 duration_ms` 经普通输入队列注入有界轨迹。
`tone prepare|play|clean` 管理独立临时 WAV，并使用正常音频服务播放。
`play` 仅排入独立诊断任务；采样工具确认音频状态已达到 `playing` 后才开始并发
测量，正常 `audio-play` 命令的完成回复不用于启动并发测试。
这些入口仅存在于测量构建，测试完成后清理 WAV。

每帧的 `display.frame`、`display.cost`、`display.input` 以 `id` 关联，另记录
shadow lease 的 `display.busy` 起止和 CPU 绘制时长。每行保持在控制台 158 字节
文本预算内。`pixel` 是首个像素 DMA 调用前的软件时间戳，**不是逻辑分析仪测得
的首个线上字节**；TE 周期同样包含 GPIO 唤醒与调度误差。物理输入时间从 CST9220
报告读取完成算起；合成轨迹从注入队列算起。输入合并时同时报告最早、最新样本，
BUSY 占比只计算输入到 LVGL 读取期间与先前 lease 的交集，不计本帧上传占用。

`scripts/device/display_probe.py capture` 保存原始控制台记录并产生分布摘要，
`analyze` 可离线重算；摘要显式列出截断记录和未配对帧。最终画面的 EmbedScope
截图仍是 RGB565，不能据此验收 RGB888 低位精度或扫描过程中的撕裂。
EmbedScope 的最终画面镜像会在每帧上传后量化／复制，尤其会延长 push 的租约。
性能 A/B 统一使用 `display-metrics` 且关闭 `embedscope`；带 EmbedScope 的截图构建
单独记录身份和证据，不能将两种构建的耗时混为一组。

上游 R-11 已在 `lv_binding_rust` 本地提交
`066f96b583e1e3dd83d6492fae961b48dd4eb8d9` 实现
`.invalidation_alignment(4, 2)` 可在 LVGL invalidation 阶段统一本板的绘制与 DIRECT
同步区。当前 PARTIAL 的持久 shadow 保证扩展列与行已初始化，未更改依赖来源，未迁移 DIRECT。

以下命令只构建。配对比较先保持 TE 策略相同，再独立比较局部低延迟实验：

```sh
# 40 MHz RGB565 + metrics
cargo fw --features display-metrics --locked --offline
# 40 MHz RGB888 + metrics，保留 TE
cargo fw --features display-rgb888,display-metrics --locked --offline
# 40 MHz RGB888 + metrics + 局部 TE 旁路
cargo fw-rgb888 --locked --offline
```

设备接入后的验收包括：

1. 纯红绿蓝、灰阶、高精度渐变、照片、透明边缘、四角小窗及非整行局部刷新，
   核对颜色通道、低位精度、源 stride、窗口边界和残影。
2. 冷启动、bootloader splash 接管、反复 sleep/wake；完整画面恢复后才恢复亮度。
3. 音量/亮度连续拖动、极短点击、松手、快速往返导航和 Home 状态更新；记录
   帧间隔及触摸到提交/首字节的 p50/p95，并检查按下/释放完整性。
4. 同格式 TE 开/关对照，采集实际 TE 周期和 SCLK，结合光学记录判断撕裂，
   不以最终画面截图替代扫描过程验收。
5. 无线与音频同时运行，记录 PSRAM/DMA 耗时、堆最低水位及最大连续块；
   失败重试、未完成 damage 和唤醒全屏恢复单独核对。

40 MHz 下全屏 RGB888 已超过 16.67 ms 的纯传输预算，当前工作不设整屏 60 FPS
承诺。局部控件的画质、输入等待和提交工作量是本轮设备对照的重点；增加 shadow、
DIRECT 或合成流水的必要性由这些设备数据决定。

## 设备对照结果（2026-11-12）

最终对照使用同一份 `d02aabd` 源码和设备上的同一套 NAND 资源，分别构建
`display-metrics`、`display-rgb888,display-metrics`，再单独增加
`display-low-latency`。所有性能构建关闭 EmbedScope，使用普通 USB 控制台；
Wi-Fi 为 idle、无 IP，蓝牙关闭，不能据此声称无线吞吐验收通过。
每种格式记录 8 秒 Home、启动器/设置往返 10 轮、音量滑块往返 10 轮及一轮音频并发 push。
滑块轨迹为 `(94,165) → (370,165)` 及反向，每程 500 ms，从普通输入队列注入。

以下均为设备日志中的分布，时间单位 ms。输入时间从合成样本入队到首个像素 DMA
调用前的软件时间戳，**不等于物理手指到线上首字节或光子的延迟**。
“最早/最新”分别对应本帧合并输入中的两个端点，不能只选其中改善的一项。

| 指标 | RGB565 | RGB888（保留 TE） |
| --- | ---: | ---: |
| Home 单次上传总耗时 p50 / p95 | 32.80 / 40.28 | 38.03 / 44.98 |
| Home 完成间隔 p50 / p95 | 51.01 / 72.65 | 49.65 / 68.31 |
| push 单次上传总耗时 p50 / p95 | 43.29 / 47.93 | 59.11 / 63.29 |
| push 完成间隔 p50 / p95 | 50.14 / 51.69 | 66.75 / 67.57 |
| 滑块每次提交字节 p50 | 40,336 | 60,504 |
| 滑块最早合并输入延迟 p50 / p95 | 89.17 / 95.18 | 86.41 / 95.22 |
| 滑块最新合并输入延迟 p50 / p95 | 48.67 / 67.65 | 46.03 / 66.82 |
| 滑块 BUSY 等待占比 p50 | 15.6% | 16.9% |
| 音频并发 push 完成间隔 p50 / p95 | 49.78 / 51.62 | 66.73 / 67.41 |

push 分别有 120 / 98 个中间帧、100 / 78 个连续完成间隔；滑块分别有
241 / 240 个能关联到合并输入的帧。上述 Home、push、slider 和音频并发日志
均无截断计量行、未配对帧或失败上传。统计保留长帧，没有按 200 ms 等任意时长删除样本。

RGB888 局部 TE 旁路的上传 p50 / p95 为 5.71 / 5.92 ms（保留 TE 为
13.33 / 20.16 ms）。最早合并输入 p50 从 86.41 降至 73.63 ms，改善 12.79 ms；
最新输入 p50 从 46.03 降至 42.33 ms，改善 3.70 ms；两者 p95 分别为
79.07 / 52.72 ms。旁路组有 279 个关联输入的帧。尚无 10 轮物理拖动的光学验收，
因此它仍是独立候选开关。BUSY 占比降至 9.3%，不满足继续引入 DIRECT / 双 shadow
的 >30% 触发条件。

阶段性串行基线 `ce09229` 的 Home 上传 p50 为 RGB565 38.56 ms、RGB888
51.87 ms；push 为 62.00 / 86.40 ms。当前总上传时间较短，但双 staging 后
pack 与 DMA 重叠并共享 PSRAM，单项 `pack_us` 并未下降；这些数据不能单独归因为
转换循环的加速。Home 完成间隔仍有长尾，未证明整体帧率稳定提高。
串行基线的动画时钟/页面准备有缺陷，不能用它的少量中间帧比较转场帧间隔。

用户在 `f0136e5` 的自动并发测试中报告转场明显跳动。日志显示某次偏移由
28 px 在约 50 ms 内跳到 464 px：诊断/脚本导航在常规 drain 之后标记 dirty，
下一轮才完成目标页面准备。`d02aabd` 在 snapshot 之后、启动动画时钟之前补齐
reconcile。重新测得 20 次 RGB565 转场每次均有 6 个中间帧，原先为 3–4 个；
修复后的完整画面序列与光学无撕裂是不同的验收项。

两种格式均从 `screen-off → restoring → active` 恢复，`frame_ready=1`；
启动时固件内观察到的 TE 周期均值约 16.69 ms，采样无超时。
测试后内部空闲堆快照 RGB565 为 96,520 B、RGB888 为 85,000 B，最大连续块
均为 65,512 B；这不是运行期最低水位或无线负载下余量。
两个并发测试均在音频进入 playing 后开始导航，结束时仍在播放，underrun / overrun
计数均为 0；最终配对两组尚无明确听感反馈，不能代替“无可闻断音”验收。

默认继续使用 RGB565、40 MHz 并保留 TE 等待。RGB888 的 push p50 66.75 ms
和 p95 67.57 ms 均未达到预先写定的 ≤50 / ≤67 ms 门槛；静态低位画质收益
也尚未获得稳定目视确认。局部控件保留高精度和按 damage 上传的实现继续保留，
默认格式无需依赖未完成的光学验收。四种候选的固件和资源预算均通过。

首次性能对照后装回的默认镜像来自 `d02aabd`，仅 `firmware` 与默认 Lua/dual-core features，
关闭 RGB888、局部 TE 旁路、metrics 和 EmbedScope。镜像为 5,363,008 B，SHA256 为
`63f040b2dabe7d233ce5837acae6785c14db6980774375cb4f70a5670e7dd770`，
已通过 Recovery 完整读回、安装后 Ready、再次重启后的 Ready 和连续三轮 2 秒灭屏唤醒。
该次安装状态为 `Ready generation=224, ready=1, sys=1, data=1, frame=1, vm=1`，
`power active, cycle=3, frame_ready=1`。测试 WAV 已删除，音量恢复至测试前的 100，
音频关闭，页面回到 Home。`final-default-r2/final-health.json` 保存最终身份和状态。

原始记录、逐项分布、构建 feature、ELF/BIN SHA256、Recovery 完整读回和 Ready
凭据保存在 `target/display-device-20261112/`：`device-comparison.json` 为汇总，
`decision-results.json` 保存固定阈值与判定，`evidence-ledger.json` 保存来源与安装凭据，
`manual-observations.json` 单独保存人工反馈。每个 `reconciled-*` 子目录包含原始日志。
实际 SCLK/脉宽、TE 到可见扫描的相位、10 轮撕裂光学记录、无线吞吐、真实 SPI
超时/面板断开和音频听感仍未验收；不会从固件时间戳或最终截图推断这些结果。

## 窗口边界与 Home 恢复回归（2026-11-12）

本节记录显示分支合并完整 Home 视口之前的源码与实机结果。

RGB888 Home 曾出现静止后仍保留的横向切割线。同一源帧冻结后，串行和并行
打包均复现；软件源画布中没有这些线。原 31 行独立 RASET 窗口违反两行边界。
改为 30 行块、damage 扩展到四列两行后，实机静止画面、动画、拖动及返回均确认
无横线。RGB888 色条的 RGB 顺序、白灰及残影检查通过。

随后导航出现另一种残留：Home 右侧留下绿白点或旧应用片段。全屏色条覆盖能够
清除；旧路径 8 轮导航的最后一轮再次出现，8 次外围清黑均报告成功。
同一源帧的串行主体重传没有清掉残留，软件源捕获也没有对应图案。
这一结果没有确定具体丢失的窗口命令或面板内部行为。

Home 完整恢复已改为 `HomeRows`，将黑背景、主体和状态带编码成同一批整宽行。
RGB888 的首帧由原来的 699,360 B / 115 窗口变为 691,200 B / 22 窗口；
普通动画仍为 354,816 B / 12 窗口，不增加全屏缓冲。
`home-restore-full-rows-r3` 的 20 轮导航完成 656 帧，日志无失败、截断或未配对记录。
目视反馈为切换瞬间闪过，停稳后自行消失，未再报告停稳后持续残留。
这不构成切换过程无撕裂验收。后续关闭 EmbedScope 的 R4 12 轮复验中，
停稳后仍报告右侧绿白点或旧应用片段；横向切割线未复发。整宽恢复本身没有
解决该现象，其收益为减少窗口命令并从同一租约恢复完整画面。

R5 区分了导航与冻结：手动返回 Home 后单独冻结，画面干净；经普通触摸输入队列
完成 10 轮往返再冻结，画面也干净。诊断 `page` 入口原本在本轮 `Ui::drain`
之后直接 `app.go`，随即进入 Home 渲染；此时 `Ui::tick` 仍遍历旧对象树，
不会替代 reconcile。入口现已移到常规 drain 前，使模型、对象树及生命周期在
渲染前完成同一轮更新。R6 保留其他条件，原直接跳页路径完成 20 轮、766 帧，
没有失败上传、截断行或未配对计量记录；最后冻结画面目视确认干净。
这些对照支持将残留按诊断入口时序回归处理，没有新增 rs_lvgl 改动需求。
冻结、同帧重传及灯光控制已从待交付源码移除。此结果不认证所有转场均无光学撕裂。

R3 使用 EmbedScope 每帧镜像，全屏 Screen/Slide job 中位耗时约 209 / 208 ms，
其中传输约 44 / 49 ms；复制和量化显著延长租约。正常转场复验关闭 EmbedScope，
不能将镜像构建的少量中间帧当作无诊断固件的转场表现。
R3 的 RTC 用量因既有 EmbedScope 记录超过常规基线 52 B，仅作为诊断构建例外，
资源策略没有更改；最终普通构建必须通过原策略。

新增测试覆盖整个 480×480 的单像素窗口扩展、两种格式的 Home 行块连续性、
奇数源行到偶数目的行的映射，以及从非黑旧页恢复完整 Home 后的每个像素。
完整 workspace 592 通过、2 ignored；RGB888 + simulator OS 库 388 通过、
2 ignored；原生 UI 4 通过，两种格式回放各 107/107 完整画面对照通过。
这些结果对应 `home-restore-rows-work/` 的源码快照与日志，性能表仍对应历史 `d02aabd`。

移除临时诊断并调整入口顺序后的 `./scripts/check.sh` 全部通过：workspace
592 通过、2 ignored，原生 UI 4 通过，Python 130 通过；驱动、Lua/TS 构建、
引擎互斥、资源包、Recovery 构建及 3,241 个 C 符号检查通过。
该脚本合计 1,131 次 Rust 测试执行通过、4 ignored，包含重复配置下的执行次数。
原始日志为 `final-production-check/check.log`，本轮修改文件 rustfmt 和 diff 检查通过。
Recovery 仅构建验证，实机安装只替换 main。

## 普通固件回归收尾（2026-11-12）

源码提交 `7910b8ac5c0a2bf10e693ff073f53d01333829da` 的普通 RGB565 和 RGB888
均已重新构建并通过原资源预算；RGB888 + simulator 库另有 388 通过、2 ignored。
构建均关闭计量、局部 TE 旁路、EmbedScope 与临时实验控制，保留 40 MHz 和 TE 等待。
`final-production-check/validation.json` 保存最终检查命令和日志指纹。

默认 RGB565 安装镜像为 5,359,376 B，SHA256：
`086472b2a0266662b8d75fe3cc4d6b291c3371482b4631cc6a423234ea73c281`。
Recovery 完整读回确认成功，安装及再次重启后均为 `Ready generation=264`、
`ready=1, sys=1, data=1, frame=1, vm=1`。三轮两秒灭屏均经过
`screen-off → restoring → active`，最终 `cycle=3, frame_ready=1`。
控制台确认 `display-test` 入口不可用，音量 100、音频 off、Wi-Fi idle、BT off。
`final-default-r3/` 保存构建身份、安装日志、重启前后 Ready 和最终健康凭据。
该普通固件的团子拖动、启动器/设置往返及返回后画面已人工确认操作正常、无残留或横线。

普通 RGB888 留存于 `final-rgb888-r7/`，镜像为 5,359,408 B，SHA256：
`4f67f6700ad4c35b2400bedfe0909b19de0ff40b1ec6c96db13254f25461eb7e`。
该普通镜像仅构建留存；本轮 RGB888 实机残留复验对应 R6 测量镜像，身份与
人工反馈见 `home-lines-investigation.json`。默认格式及前述未测光学项目的判定未改变。

## 完整 Home 视口与爬行动作合并（2026-11-12）

`codex/dango-v0.5.1` 的最终实现 `743dd55` 包含宽松换侧识别、固定路线爬行、
原尺寸旋转、快速半圈掉落与配套惊慌表情。完整视口与连接图标在同一帧合成；
显示分支的两行窗口、DMA 打包重叠、LVGL 局部 damage、页面准备与诊断入口时序
继续执行原有路径。两类来源都通过 `PixelSource` 明确像素格式、尺寸与行距。

Home 每次上传完整视口，单帧像素量相对原局部主体有所增加。前述性能表对应
历史 `d02aabd`，其 Home 上传时长、完成间隔和局部字节数不代表本次完整视口。
默认仍为 RGB565、40 MHz、保留 TE 等待。

图标回归使用实际 `FoxWidget` 发布帧，在 RGB565 与 RGB888 下执行驱动的编码和
行块规划，逐窗口检查连接图标，完整上传后逐字节对照源帧，覆盖从非黑旧页返回
Home 的恢复。合并与安装证据保存在 `target/merge-companion-20261112/`。

合并检查的 workspace 测试为 597 通过、2 ignored；完整 `scripts/check.sh` 合计
1,141 次 Rust 测试执行通过、4 ignored，包含不同引擎配置重复执行，Python 为
130 通过。RGB888 + simulator 库另有 393 通过、2 ignored；两种面板格式的页面
回放各完成 107 次完整画面对照，计量、局部 TE 旁路与 EmbedScope 组合的设备
交叉编译通过。70°换侧及快速半圈各生成 110 帧模拟 IMU 原生预览，像素结果不
代表实体传感器方向或光学帧率验收。

普通 RGB565 实机验收对应合并源码 `31d85d527a761e2dc1a0f77301de771c1fa74c9d`。
Recovery 完整读回验证通过；安装及再次重启后均为 `Ready generation=269`，
三轮灭屏唤醒通过，四套皮肤均可加载，八个设备资源文件与合并源码逐字节一致。
人工慢转约 70–100°后，角色保持原尺寸爬到新侧并停稳；快速半圈后能完成
惊慌抓握、掉落压扁、无奈和恢复的完整动作，画面正常。该反馈覆盖上述两种
动作的实体表现；普通 RGB888 仍仅构建留存，完整视口的实机帧率与光学撕裂未重新测量。
