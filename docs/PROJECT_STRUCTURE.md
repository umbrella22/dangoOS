# 源码结构

workspace 有三个 crate：

| crate | 作用 |
| --- | --- |
| `crates/mosaico-s31` | **库**（`src/lib.rs`，`no_std + alloc`，能在电脑上 `cargo test`）+ **固件**（`src/main.rs`，只为 `riscv32imafc-unknown-none-elf` 构建） |
| `crates/mosaico-lua` | Lua 5.5.1 与官方 esp-claw 模块的移植层；C 源码用 `cc` 编译，主机测试用同一份 C |
| `crates/mosaico-fs` | littlefs 2.11.3 + 挂载表 VFS + newlib 文件系统调用，Lua 的 `io` / `storage` 走它 |

规则很简单：**不碰硬件的代码放库里，碰硬件的放固件里**。库里的东西都有 host 测试；
固件只做外设驱动和任务编排。

## 一帧的数据流

```text
触摸 / IMU / 磁力计 / 电量 / 模块按键
        │  tasks::input、tasks::sensors（固件）
        ▼
behavior::Runtime ── affect（传感器→情绪）+ personality（点按阶梯）→ Expression + 马达 + 灯光
        │  tasks::ui 每 5 ms 驱动（固件）
        ▼
hub ── 当前页面：主页 / 应用网格 / 原生应用 / Lua 应用（边缘手势切换）
        ├─ 主页：fox::FoxAnimator → fox::Scene → fox::FoxRenderer → ui::canvas_pool 的空闲槽
        ├─ 原生应用与网格：ui::hub_view（lvgl-ui）→ drivers::lvgl_port 的 PSRAM 影子帧
        └─ Lua 应用：tasks::lua 的 display 画布 → 影子帧
        ▼
tasks::plane（中断执行器）── TE 同步 + 32 行 QSPI DMA → drivers::display → CO5300

USB CDC ── console：日志泵 + hub::push 推送协议 → /data/apps → tasks::apps 重新扫描 → hub::registry
```

渲染管线的细节见 [FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)。

## 目录

```text
.
├── Cargo.toml / Cargo.lock      workspace（三个成员）；esp-hal 一族与 lvgl 的 Git rev 固定在这里
├── .cargo/config.toml           cargo fw 别名、交叉编译工具链路径
├── rust-toolchain.toml          stable + riscv32imafc 目标
├── assets/
│   ├── fox-dango/               角色素材与 Python 管线（生成 src/fox/data.rs、src/assets/fox.bin）
│   └── fonts/                   Hub 中文点阵字体生成脚本
├── examples/lua-apps/hello_push 推送示例应用（弹球 + io 计数器）
├── scripts/
│   ├── check.sh                 不需要板子的全部检查
│   ├── test-drivers.sh          纯驱动模块的 rustc --test
│   ├── check-rom-symbols.sh     vendored C 与 S31 ROM 符号冲突检查
│   ├── flash-app.sh             构建并只写 main_app（0x210000）
│   └── push-app.py              USB 推送 Lua 应用、热重载、远程重启
├── docs/
│   ├── HANDOFF.md               交接文档：现状、下一步、后续拓展（从这里开始）
│   ├── PROJECT_STRUCTURE.md     本文
│   ├── DEVELOPMENT.md           开发、检查、烧录、串口、推送
│   ├── MOSAICO_V12_HARDWARE.md  引脚、电源、扩展槽
│   ├── FOX_RENDER_PIPELINE.md   渲染管线学习笔记
│   ├── FOX_DRAWING_GUIDE.md     角色美术怎么画、怎么画新角色
│   ├── ASSET_REQUESTS.md        素材规格
│   ├── plans/                   进行中的 RFC：hub-shell-plan（RFC 0006）、lua-runtime-plan
│   └── lessons/                 真机调试复盘（现象、日志、原因、经验）
└── crates/
    ├── mosaico-fs/
    │   ├── src/                 lfs.rs（一个 littlefs）、vfs.rs（挂载表与描述符）、global.rs（固件全局 VFS 与 C 入口）
    │   ├── csrc/                littlefs 胶水与 POSIX 系统调用
    │   └── vendor/littlefs-2.11.3
    ├── mosaico-lua/
    │   ├── src/                 vm.rs（一个脚本一个 VM，协作式让出）、call.rs、platform.rs、jpeg.rs、ffi.rs
    │   ├── csrc/                平台层、loslib、TJpgDec 胶水、esp_shim（ESP-IDF 头文件替身）
    │   ├── vendor/              lua-5.5.1、esp-claw 模块、cjson、tjpgd、skills-lab 应用（来源见 NOTICE.md）
    │   └── testdata/
    └── mosaico-s31/
        ├── examples/
        │   ├── preview.rs       表情在 480x480 LVGL host 显示上的预览
        │   ├── hub_preview.rs   Hub 各页面的主机截图
        │   └── splash_preview.rs  启动页与入场动画的 PPM 帧
        └── src/
            ├── lib.rs           库入口：模块表与常用 re-export
            ├── assets/          编译期 include_bytes! 的二进制资源（见其 README）
            │   ├── fox.bin      狐狸全部图像的行 RLE（fox_pack.py 生成）
            │   ├── hub-*.bin    Hub 中文点阵字体
            │   └── bmi270-config.bin  Bosch BMI270 配置微码
            ├── behavior/        “狐狸怎么想”
            │   ├── mod.rs       Expression
            │   ├── affect.rs    传感器事件 → 情绪 / 表情 / 马达（顶部表格是规则说明）
            │   ├── magnet.rs    磁力计基线学习与接近/接触/旋转分类
            │   ├── mood_light.rs  交互板 6 颗 WS2812：表情 → 颜色与动态
            │   ├── personality.rs 触摸 → 点按阶梯（好奇 → 疑惑 → 不耐烦 → 生气）
            │   ├── runtime.rs   合并两者：紧急情绪 > 触摸反应 > 情绪表情 > 静息
            │   └── tests/       按场景分的时间线测试
            ├── fox/             “狐狸长什么样、怎么动”
            │   ├── mod.rs       Img、FoxPose、Expression → 姿势映射
            │   ├── data.rs      生成的调色板与偏移表（图像本体在 assets/fox.bin）
            │   ├── scene.rs     Scene / Layer / Warp：一帧要画什么
            │   ├── animate.rs   FoxAnimator：表情 + 时间 → Scene
            │   ├── render.rs    FoxRenderer：Scene → RGB565
            │   ├── entrance.rs  开机下落、压扁、小跳
            │   └── math.rs      无 libm 的 sin / exp / 反平方根
            ├── hub/             Hub 外壳（纯逻辑）
            │   ├── mod.rs       页面导航与应用状态
            │   ├── gesture.rs   边缘手势
            │   ├── transition.rs  页面滑动
            │   ├── countdown.rs 计时器（后台继续走）
            │   ├── snapshot.rs  传感器最新值
            │   ├── json.rs      launcher.json 读取
            │   ├── registry.rs  Lua 应用注册表（/data/apps、/system/apps、内置）
            │   └── push.rs      USB 推送协议状态机
            ├── ui/
            │   ├── mod.rs       LVGL 屏幕树
            │   ├── fox_widget.rs  FoxControl（共享状态）与 FoxWidget
            │   ├── canvas_pool.rs 两槽画布：渲染与 DMA 的所有权交接
            │   ├── hub_view.rs  应用网格与设置 / 传感器 / 计时器视图
            │   ├── hub_font.rs  中文点阵字体
            │   └── splash/      ikaros 字标（当前关闭）
            ├── telemetry/       延迟与显示计数器（串口统计行用）
            ├── main.rs          固件入口：上电、外设初始化、启动任务
            ├── board.rs         v1.2 引脚表与板版本检查
            ├── console.rs       USB CDC-ACM：日志队列与输出泵、推送命令接收
            ├── crashlog.rs      panic 记录存 RTC 内存，下次启动打印并跳过出事阶段
            ├── tasks/           Embassy 任务，每个独占自己的硬件
            │   ├── plane.rs     上屏（中断执行器）
            │   ├── ui.rs        LVGL + behavior 运行时 + Hub + 渲染
            │   ├── lua.rs       Lua 应用运行器（时间片、内存上限）
            │   ├── apps.rs      设备上的应用扫描与对 UI 任务的请求
            │   ├── input.rs     触摸读取
            │   ├── sensors.rs   IMU、磁力计、电量、左右扩展槽
            │   ├── haptic.rs    马达（RMT ch0 生成 PWM）
            │   └── lights.rs    交互板 WS2812（RMT ch1，右槽 GPIO38）
            └── drivers/         外设驱动
                ├── display.rs   CO5300 QSPI + DMA（display/packing.rs：窗口打包）
                ├── touch.rs     CST9220（touch/protocol.rs：报告解码）
                ├── touch_sens.rs  S31 电容触摸外设（寄存器级，交互板按键）
                ├── imu.rs       BMI270 + BMM150
                ├── battery.rs   BQ27220
                ├── storage.rs   flash 访问（esp_storage::ll）、分区表解析、挂载 /system 与 /data
                ├── subboard/    左右扩展槽（探测、交互板、EEPROM 描述符、去抖、触摸键）
                ├── shared_i2c.rs  I2C0 / I2C1 异步互斥
                ├── geometry.rs  面板尺寸与窗口类型
                ├── memory.rs    稳定地址的像素内存
                ├── libc.rs      C 代码需要的 libc 符号
                └── lvgl_port.rs LVGL 显示与指针：分块渲染进 PSRAM 影子帧、页面滑动快照
```

## 依赖方向

- `behavior::{affect, personality}` 不依赖任何其他模块，是纯状态机。
  `behavior::runtime` 是唯一的胶水：它把合并后的表情和姿态写进 `ui::fox_widget::FoxControl`。
- `fox` 只依赖 `behavior::Expression`。
- `hub` 依赖 `behavior`（事件、手势采样）和 `mosaico-fs`（注册表、推送写文件），不依赖 `ui`。
- `ui` 依赖 `behavior`、`fox` 和 `hub`。
- `mosaico-lua`、`mosaico-fs` 不依赖 `mosaico-s31`；固件通过 host 钩子（显示、时间、触摸）注入设备能力。
- `tasks` 依赖库里的一切，加上 `drivers`。
- 库永远不依赖固件（`tasks`、`drivers`、`board`、`console`）。需要设备时钟之类的东西时，
  由固件注入（例如 `fox_widget::install_mono_us`）。

## 测试在哪里

| 范围 | 命令 |
| --- | --- |
| 库（behavior、fox、ui、hub、telemetry） | `cargo test -p mosaico-s31 --offline` |
| Lua 运行时与模块 | `cargo test -p mosaico-lua --offline` |
| 文件系统 | `cargo test -p mosaico-fs --offline` |
| 纯驱动模块（触摸解码、窗口打包、几何、槽位描述符、去抖与 LDR 换算） | `./scripts/test-drivers.sh` |
| 全部（含示例、固件构建和 ROM 符号检查） | `./scripts/check.sh` |

固件 bin 本身设置了 `test = false`，因为它只能为 RISC-V 目标编译。
