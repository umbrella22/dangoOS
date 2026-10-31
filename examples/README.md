# 开发示例

| 目录 | 用途 |
| --- | --- |
| `apps/hello` | 最小 Lua 应用：本地模块、绘制、延时和独立存储 |
| `apps/hardware` | 读取硬件快照，按参数演示按键、灯光和马达接口 |
| `brains/happy.lua` | 常驻行为脚本的最小示例 |

应用示例使用与正式应用相同的 manifest 和入口约定，但不进入 system 镜像。
正式应用归入 [apps](../apps/README.md)。

```sh
python3 scripts/device/console.py push examples/apps/hello --run --follow
python3 scripts/device/console.py push examples/apps/hardware --run --follow
python3 scripts/device/console.py brain-push examples/brains/happy.lua
```

硬件示例默认只读取；`args.outputs=true` 启用灯光和马达请求。接口与资源清理约定
见 [Lua 应用开发](../docs/guides/lua-apps.md) 和 [硬件接口](../docs/guides/lua-hardware.md)。
