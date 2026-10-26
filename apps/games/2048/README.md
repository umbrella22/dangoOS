# 2048

通过滑动合并相同数字的方块并累积得分。界面显示当前分数和最高分；无可用移动时
可以重新开始。从设备应用列表打开，使用系统返回操作退出。

应用 ID 为 `2048`，入口为 `scripts/main.lua`，需要触摸显示。上游脚本提供可选音频
处理；具体设备能否播放声音取决于宿主提供的模块。

参数放在 `launcher.json.args`，默认值即可运行。`target_size` 调整棋盘大小；
`run_time_ms` 指定运行毫秒数，`0` 表示持续运行。

默认版本随 system 镜像交付，开发时可从仓库根目录更新：

```sh
python3 scripts/device/console.py push apps/games/2048 --run --follow
```

源码来自 `espressif/esp-claw-skills-lab` 的
`57969b1e5891b5ad68e3d2433f1e9dde3ab1fcc0`，MIT 许可证见 `LICENSE`。应用目录约定见
[应用总览](../../README.md)，实际设备观察见
[2026-10-26 刷入记录](../../../docs/history/records/device-flash-20261026.md)。
