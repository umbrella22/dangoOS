# 贪吃蛇自动演示

持续运行的自动寻路贪吃蛇。脚本自行选择方向和寻找食物，当前没有手动方向控制。

从设备应用列表打开，使用系统返回操作退出。应用 ID 为 `snake`，入口为
`scripts/main.lua`，需要显示模块。

参数放在 `launcher.json.args`，默认值即可运行。可以调整每帧间隔、运行时长、格子
大小、边界穿越和随机种子；颜色由脚本读取对应参数。`duration_ms=0` 表示持续运行。

默认版本随 system 镜像交付，开发时可从仓库根目录更新：

```sh
python3 scripts/device/console.py push apps/games/snake --run --follow
```

源码来自 `espressif/esp-claw-skills-lab` 的
`57969b1e5891b5ad68e3d2433f1e9dde3ab1fcc0`，MIT 许可证见 `LICENSE`。应用目录约定见
[应用总览](../../README.md)，实际设备观察见
[2026-10-26 刷入记录](../../../docs/history/records/device-flash-20261026.md)。
