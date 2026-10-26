# 工程脚本

根目录只保留统一检查入口 `check.sh`，其他脚本按用途归类。命令从仓库根目录执行。

| 位置 | 用途 |
| --- | --- |
| `check.sh` | workspace、应用包、Python、驱动、固件和 ROM 符号检查 |
| `check/drivers.sh` | 独立 Rust 驱动测试 |
| `check/rom-symbols.sh` | 构建产物与芯片 ROM 的 C 符号冲突检查 |
| `check/ota-corpus.py` | 从固件生成 OTA 校验的异常镜像测试语料 |
| `device/console.py` | 应用、照片、皮肤、行为脚本和 OTA 的 USB 控制台操作 |
| `device/session.py` | 有时限的控制台会话和日志采集 |
| `device/flash.sh` | 明确选择 slot 0 的 ROM 恢复操作 |
| `device/layout.py` | 分区备份、A/B 布局准备与恢复 |
| `device/ota_image.py` | 供设备工具共用的离线固件镜像校验器 |
| `device/vendor` | 原样保留的 ESP-IDF 分区生成器及许可证 |
| `assets/photo-icon.cjs` | 从 SVG 生成 Photo 的启动器图标 |

脚本的离线回归放在 [tests/host](../tests/README.md)，设备探针放在
[tests/device](../tests/device/README.md)。应用镜像打包由 Rust 主机工具
[app-pack](../tools/app-pack/README.md) 提供。

```sh
./scripts/check.sh
python3 -B -m unittest discover -s tests/host
python3 scripts/device/session.py --help
```

设备操作工具需要 pyserial / esptool；素材脚本需要 Node.js 和 Sharp。统一检查不连接
设备。具体构建与交付命令见 [开发指南](../docs/guides/development.md)。
