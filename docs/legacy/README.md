# 历史参考

该目录保存早期代码草稿和设计记录，**不参与 Cargo 构建**。

| 路径 | 内容 |
| --- | --- |
| `embedded-hal/` | 原 `src/embedded_hal/`，包含未完成的显示驱动和早期多板型草稿 |
| `service/` | 原 `src/service/` 中未启用的硬件占位与动画模块 |
| `PROJECT_COMPLETE.md`、`PROJECT_SUMMARY.md`、`VARIANT_SUMMARY.md` | 历史设计及进度记录 |

这些记录保留原内容，其中的构建命令、目录路径、性能宣称和“完成”状态不代表当前实现。早期 Korvo 引脚取自另一块 S3 板，不能用于 S31-Korvo-1 接线。

当前源码在 [`crates/`](../../crates/)，结构见 [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md)，硬件配置见 [KORVO_PINS.md](../KORVO_PINS.md)。
