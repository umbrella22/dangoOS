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
| P3 | 交互模块其余外设 | WS2812、SI12T、红外（RFC 0004 未开工部分） |
| P3 | 启动页 | `SHOW_SPLASH=false`；官方点阵字体见 `docs/ASSET_REQUESTS.md` |
| 另立项 | Hub / C3 / 语音 / Lua | Listening/Thinking/Speaking 姿势已就绪，但还没有语音来源驱动；需新 RFC |
| 延后 | RFC 0001 组件库 | 不在产品关键路径上 |

## 约束

- 烧录只写 main_app `0x210000`；不碰 bootloader、分区表和 `0x20000` vibe_mode。
- `lv_binding_rust` 固定在 `lvgl.rev`，用 `scripts/check-lvgl-pin.sh` 检查。
