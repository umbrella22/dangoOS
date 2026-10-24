# 调试复盘

每次真机调试后的经验记录：现象、日志、原因、修复，以及可以带走的通用做法。

| 日期 | 主题 | 文档 |
| --- | --- | --- |
| 2026-10-22 | 交互板电容触摸键（S31 touch，寄存器级驱动） | [touch-keys.md](touch-keys.md) |
| 2026-10-22 | L2 上机：开机循环重启（ROM 符号冲突）、flash 大小识别为 0 | [l2-storage-boot.md](l2-storage-boot.md) |
| 2026-10-23 | N1–N7 测试前核对：复盘保护、马达对照与震动取消修复 | [n1-n7-preflight.md](n1-n7-preflight.md) |
| 2026-10-23 | CoreBoard V1.2 官方指南核对、100% 马达编码、USB 自动复位与换槽约束 | [esp-mosaico-v1.2-reference.md](esp-mosaico-v1.2-reference.md) |
| 2026-10-24 | POWER 开机后右槽描述符首读失败，有限重试与验收边界 | [subboard-descriptor-retry.md](subboard-descriptor-retry.md) |

前两篇记录当时的实现和设备观察。当前布局与刷机约束以 [HANDOFF](../HANDOFF.md#红线)
为准；历史上的分区末端容量推算、工厂固件回退现象不作为新 A/B 布局的验收证据。
