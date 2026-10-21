# 源码结构

仓库只有一个 crate：`crates/mosaico-s31`。它同时是

- **库**（`src/lib.rs`，`mosaico_s31`）：`no_std + alloc`，能在电脑上 `cargo test`；
- **固件**（`src/main.rs`，`mosaico-s31-fw`）：只为 `riscv32imafc-unknown-none-elf` 构建。

规则很简单：**不碰硬件的代码放库里，碰硬件的放固件里**。库里的东西都有 host 测试；
固件只做外设驱动和任务编排。

## 一帧的数据流

```text
触摸 / IMU / 磁力计 / 电量 / 模块按键
        │  tasks::input、tasks::sensors（固件）
        ▼
behavior::Runtime ── affect（传感器→情绪）+ personality（点按阶梯）→ Expression + 马达
        │  tasks::ui 每 5 ms 驱动（固件）
        ▼
ui::fox_widget::FoxControl ── 表情、入场、脉冲
        ▼
fox::FoxAnimator → fox::Scene → fox::FoxRenderer ── 写入 ui::canvas_pool 的空闲槽
        ▼
tasks::plane（中断执行器）── TE 同步 + 32 行 QSPI DMA → drivers::display → CO5300
```

渲染管线的细节见 [FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)。

## 目录

```text
.
├── Cargo.toml / Cargo.lock      workspace（唯一成员 crates/mosaico-s31）
├── .cargo/config.toml           cargo fw 别名、交叉编译工具链路径
├── rust-toolchain.toml          stable + riscv32imafc 目标
├── lvgl.rev                     ../lv_binding_rust 的已测版本
├── assets/fox-dango/            角色素材与 Python 管线（生成 src/fox/data.rs、src/assets/fox.bin）
├── scripts/
│   ├── check.sh                 不需要板子的全部检查
│   ├── test-drivers.sh          纯驱动模块的 rustc --test
│   ├── flash-app.sh             构建并只写 main_app（0x210000）
│   └── check-lvgl-pin.sh        检查 lv_binding_rust 版本
├── docs/
│   ├── FOX_RENDER_PIPELINE.md   渲染管线学习笔记
│   ├── MOSAICO_V12_HARDWARE.md  引脚、电源、扩展槽
│   ├── ASSET_REQUESTS.md        素材规格
│   ├── plans/                   RFC 与执行记录（历史，含 grok-ball 阶段）
│   └── legacy/                  风扇 / Slint 时代的文档
└── crates/mosaico-s31/
    ├── examples/
    │   ├── preview.rs           16 表情在 480x480 LVGL host 显示上的预览
    │   └── splash_preview.rs    启动页与入场动画的 PPM 帧
    └── src/
        ├── lib.rs               库入口：模块表与常用 re-export
        ├── assets/              编译期 include_bytes! 的二进制资源（见其 README）
        │   ├── fox.bin          狐狸全部图像的行 RLE（fox_pack.py 生成）
        │   └── bmi270-config.bin  Bosch BMI270 配置微码
        ├── behavior/            “狐狸怎么想”
        │   ├── mod.rs           Expression（18 个表情）
        │   ├── affect.rs        传感器事件 → 情绪 / 表情 / 马达（摇晃、光线、PIR、磁铁、电量…）
        │   ├── magnet.rs        磁力计基线学习与接近/接触/旋转分类
        │   ├── mood_light.rs    交互板 6 颗 WS2812：表情 → 颜色与动态
        │   ├── personality.rs   触摸 → 点按阶梯（好奇 → 疑惑 → 不耐烦 → 生气）
        │   ├── runtime.rs       合并两者：紧急情绪 > 触摸反应 > 情绪表情 > 静息
        │   └── tests/           按场景分的时间线测试
        ├── fox/                 “狐狸长什么样、怎么动”
        │   ├── mod.rs           Img、FoxPose、Expression → 姿势映射
        │   ├── data.rs          生成的调色板与偏移表（图像本体在 assets/fox.bin）
        │   ├── scene.rs         Scene / Layer / Warp：一帧要画什么
        │   ├── animate.rs       FoxAnimator：表情 + 时间 → Scene
        │   ├── render.rs        FoxRenderer：Scene → RGB565
        │   ├── entrance.rs      开机下落、压扁、小跳
        │   └── math.rs          无 libm 的 sin / exp / 反平方根
        ├── ui/                  LVGL 屏幕树
        │   ├── mod.rs           MosaicoApp（启动页 → 狐狸）
        │   ├── fox_widget.rs    FoxControl（共享状态）与 FoxWidget（LVGL 自定义控件）
        │   ├── canvas_pool.rs   两槽画布：渲染与 DMA 的所有权交接
        │   └── splash/          ikaros 字标（当前关闭）
        ├── telemetry/           延迟与显示计数器（串口统计行用）
        ├── main.rs              固件入口：上电、外设初始化、启动任务
        ├── board.rs             v1.2 引脚表与板版本检查
        ├── console.rs           USB CDC-ACM 日志队列与输出泵
        ├── tasks/               Embassy 任务，每个独占自己的硬件
        │   ├── plane.rs         上屏（中断执行器）
        │   ├── ui.rs            LVGL + behavior 运行时 + 渲染
        │   ├── input.rs         触摸读取
        │   ├── sensors.rs       IMU、磁力计、电量、左右扩展槽
        │   ├── haptic.rs        马达（RMT ch0 生成 PWM）
        │   └── lights.rs        交互板 WS2812（RMT ch1，右槽 GPIO38）
        └── drivers/             外设驱动
            ├── display.rs       CO5300 QSPI + DMA（display/packing.rs：窗口打包）
            ├── touch.rs         CST9220（touch/protocol.rs：报告解码）
            ├── imu.rs           BMI270 + BMM150
            ├── battery.rs       BQ27220
            ├── subboard/        左右扩展槽
            │   ├── mod.rs       槽位探测、模块分类、交互板仲裁
            │   ├── interaction.rs  一侧交互板的引脚（LDR/PIR/双键）
            │   ├── descriptor.rs   EEPROM 描述符解析（纯逻辑，host 测试）
            │   └── signal.rs    去抖与 LDR 换算（纯逻辑，host 测试）
            ├── shared_i2c.rs    I2C0 / I2C1 异步互斥
            ├── geometry.rs      面板尺寸与窗口类型
            ├── memory.rs        稳定地址的像素内存
            └── lvgl_port.rs     无头 LVGL 显示（只为创建对象）
```

## 依赖方向

- `behavior::{affect, personality}` 不依赖任何其他模块，是纯状态机。
  `behavior::runtime` 是唯一的胶水：它把合并后的表情和姿态写进 `ui::fox_widget::FoxControl`。
- `fox` 只依赖 `behavior::Expression`。
- `ui` 依赖 `behavior` 和 `fox`。
- `tasks` 依赖库里的一切，加上 `drivers`。
- 库永远不依赖固件（`tasks`、`drivers`、`board`、`console`）。需要设备时钟之类的东西时，
  由固件注入（例如 `fox_widget::install_mono_us`）。

## 测试在哪里

| 范围 | 命令 |
| --- | --- |
| 库（behavior、fox、ui、telemetry） | `cargo test -p mosaico-s31 --offline` |
| 纯驱动模块（触摸解码、窗口打包、几何、槽位描述符、去抖与 LDR 换算） | `./scripts/test-drivers.sh` |
| 全部（含示例和固件构建） | `./scripts/check.sh` |

固件 bin 本身设置了 `test = false`，因为它只能为 RISC-V 目标编译。
