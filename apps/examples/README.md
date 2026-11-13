# 开发示例

| 目录 | 用途 |
| --- | --- |
| `hello` | 最小 Lua 应用：本地模块、绘制、延时和独立存储 |
| `hardware` | 查询能力、服务状态和真实堆统计，按参数演示按键、灯光和马达接口 |

应用示例使用与正式应用相同的 manifest 和入口约定，但不进入 system 镜像。
正式应用归入 [apps](../README.md)。

```sh
python3 tools/device/console.py push apps/examples/hello --run --follow
python3 tools/device/console.py push apps/examples/hardware --run --follow
```

硬件示例默认只读取；`args.outputs=true` 启用灯光和马达请求。
`args.probe_heap=true` 在启动时各执行一次 Internal 32 KiB、PSRAM 256 KiB、128 字节对齐的
真实申请/归还探针。探针结果只覆盖当时的尺寸和对齐，会扰动空闲链表顺序，不作持续采样。
接口与资源清理约定
见 [Lua 应用开发](../../docs/guides/lua-apps.md) 和 [硬件接口](../../docs/guides/lua-hardware.md)。

TS 源码示例在独立 [EmbedTypescript/examples](https://github.com/umbrella22/EmbedTypescript/tree/main/examples) 维护，构建为 `.etb` 后由 OS 安装。
