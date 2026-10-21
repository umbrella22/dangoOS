# 现状与剩余工作

> 2026-10-21 更新。仓库只剩 `crates/mosaico-s31`，结构见
> [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md)。grok-ball 时代的执行顺序、
> R0/R1/G/H 阶段门和撕裂排查记录已归档到
> [legacy/plans/ROADMAP-2026-10-20.md](../legacy/plans/ROADMAP-2026-10-20.md)。

## 已完成并真机验收

- 狐狸团子 20 个姿势（含 peering/dazzled）+ 部件动画，固定画布 19 fps。
- 光线遮挡/强光、拿起/持续晃/狂甩、按住/滑动抚摸（`4e558ff`）。
- PIR 来访：低电平短于 30 s 算同一次来访，只打一次招呼（`253580d`）。
- IMU 改读 BMI270 FIFO，稳定 50–52 Hz、`batch=2`、无丢帧（`253580d`）。
- 马达波形手感。
- 左右槽自适应交互模块；相机插入 → Curious、拔出 → Confused。
- 磁铁贴上 → Satisfied、拿开 → Confused（4/4 次准确）。插拔模块不屏蔽光线反应（有意保留）。

## 已完成，待真机复核

- 磁力计基线（`9169f90`，`behavior/magnet.rs`）：开机从 8 个安静样本（跨度 ≤ 12 µT）学基线，
  磁场还在动就继续等；任一槽位插拔后 800 ms 重新学，避免相机/模块里的磁铁抬高基线。
  串口应出现 `sensor mag[i] baseline X uT (rejected windows: n)`。
  思路参考官方 `mosaico_mag_classifier.c` 的开机滤波 + 跨度门。

## 剩余工作

| 优先级 | 项目 | 说明 |
| --- | --- | --- |
| P1 | 真机验收补齐 | 电量 Tired/Dying；磁力计新基线日志 |
| P2 | 触摸延迟复测 | 换成狐狸后未重测 100 次触摸 P95 |
| P1 | WS2812 心情灯 | 已实现，待真机验收：右槽 GPIO38、RMT ch1 10 MHz；每个表情一种颜色 + 活泼动态，按环境光调亮度，按键点亮同侧（`behavior/mood_light.rs`、`tasks/lights.rs`）。左槽 GPIO15 与相机 D1 共用，暂不支持 |
| P3 | 电容触摸键 | 官方用 S31 自带 touch sensor（不是 SI12T）。先确认 esp-hal 在 S31 上是否有 touch 驱动，没有就要自写寄存器驱动；现在 GPIO 按键模式可用 |
| 搁置 | 红外发射 | 模块只有发射管（GPIO48/47，RMT NEC），没有接收头，学不了码；家里设备的码未知。重启条件：按品牌查到 NEC 码表，或用另一块带接收头的板子录码。用法设想：狐狸困了发“关灯”、生气时给电视发静音 |
| P3 | 启动页 | `SHOW_SPLASH=false`；官方点阵字体见 `docs/ASSET_REQUESTS.md` |
| 另立项 | Hub / C3 / 语音 / Lua | Listening/Thinking/Speaking 姿势已就绪，但还没有语音来源驱动；需新 RFC |
| 延后 | RFC 0001 组件库 | 不在产品关键路径上 |

## 约束

- 烧录只写 main_app `0x210000`；不碰 bootloader、分区表和 `0x20000` vibe_mode。
- `lv_binding_rust` 固定在 `lvgl.rev`，用 `scripts/check-lvgl-pin.sh` 检查。
