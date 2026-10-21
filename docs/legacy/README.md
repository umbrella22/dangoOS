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

当前源码结构见 [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md)，硬件见 [MOSAICO_V12_HARDWARE.md](../MOSAICO_V12_HARDWARE.md)。
