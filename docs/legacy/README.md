# 历史参考

该目录保存风扇控制 / Slint 时代的代码草稿、设计与发布记录，**不参与 Cargo 构建**。

| 路径 | 内容 |
| --- | --- |
| `embedded-hal/` | 原 `src/embedded_hal/`，未完成的显示驱动和早期多板型草稿 |
| `service/` | 原 `src/service/` 中未启用的硬件占位与动画模块 |
| `PROJECT_COMPLETE.md`、`PROJECT_SUMMARY.md`、`VARIANT_SUMMARY.md` | 历史设计及进度记录 |
| `MOSAICO_UI_DESIGN.md`、`MOSAICO_UI_ROADMAP.md`、`MOSAICO_UI_REVIEW.md`、`mosaico-ui-preview.html`、`eve-fan-control-ultimate.html` | Slint 版 Mosaic UI 的设计、路线图与验收 |
| `TREE_SHAKING.md` | fan crates 的 feature 边界 |
| `releases/` | fan-control v14–v22 发布说明（app 写在 `0x20000`，与现在的 `0x210000` 不同） |

这些记录保留原内容，其中的构建命令、目录路径、烧录地址和“完成”状态不代表当前实现。

## 当时规划、现在去向

从 `MOSAICO_UI_ROADMAP.md` 和 `MOSAICO_UI_DESIGN.md` 整理，状态按当前代码（狐狸团子固件）核对：

| 当时的规划 | 现在 |
| --- | --- |
| 按官方 esp-mosaico-claw 场景重做 Hub（Home/Apps 横滑、下拉面板、锁屏、Settings） | 未迁移。当前首屏只有狐狸；Hub 设计规格仍可作参考 |
| 左槽相机预览（OV3640 RGB565 / SC101IOT UYVY，AXI DMA，完整帧校验） | 驱动未迁移。`drivers/subboard` 已能识别相机（0x07，固定按左槽）并释放 GPIO14；采集仍待做，硬件要点见 MOSAICO_V12_HARDWARE.md §7.3 |
| 扩展槽识别与 GPIO 仲裁（当时“通用模块发现与仲裁仍待实现”） | 已实现：左右槽探测、描述符校验、交互板任一侧可用、同时只读一块 |
| IMU 水平仪页 | 未做页面；IMU 数据现在驱动狐狸的摇晃/倾斜反应 |
| TOF 测距页、NAND 相册、Breakout 打砖块 | 未迁移 |
| 天气 / NTP | 仍受阻：Rust 生态暂无 S31 的 esp-wifi |
| 风扇数据卡 / FAN_PWM(GPIO12) | 已放弃风扇方案，GPIO12 现在是左槽交互板 KEY_R |
| Lua 工具层 | 未立项 |

当前源码结构见 [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md)，硬件见 [MOSAICO_V12_HARDWARE.md](../MOSAICO_V12_HARDWARE.md)。
