# 工程脚本

根目录只保留统一检查入口 `check.sh`，其他脚本按用途归类。命令从仓库根目录执行。

| 位置 | 用途 |
| --- | --- |
| `check.sh` | workspace、应用包、Python、驱动、固件和 ROM 符号检查 |
| `check/drivers.sh` | 独立 Rust 驱动测试 |
| `check/rom-symbols.sh` | 构建产物与芯片 ROM 的 C 符号冲突检查 |
| `check/main-corpus.py` | 从固件生成 主镜像校验的异常镜像测试语料 |
| [release/](release/README.md) | tag 发布、固定版本工具链、主应用与全量安装包生成及附件回读校验 |
| `device/console.py` | 应用、照片、皮肤、行为脚本和救援切换的 USB 控制台操作 |
| `device/recovery.py` | RCR1 只读导出、显式主应用安装和软件 ROM 入口 |
| `device/agent.py` | 电脑 Responses 后端、设备工具和生成技能闭环 |
| `device/session.py` | 有时限的控制台会话和日志采集 |
| `device/flash.sh` | 构建主应用并通过 Recovery USB 安装 |
| `device/layout.py` | 完整 NOR 备份、单主应用布局配置与恢复 |
| `device/main_image.py` | 供设备工具共用的 S31ONE03 主镜像校验器 |
| `device/vendor` | 原样保留的 ESP-IDF 分区生成器及许可证 |
| `assets/photo-icon.cjs` | 从 SVG 生成 Photo 的启动器图标 |

脚本的离线回归放在 [tests/host](../tests/README.md)。应用镜像打包由 Rust 主机工具
[app-pack](../tools/app-pack/README.md) 提供。

```sh
./scripts/check.sh
python3 -B -m unittest discover -s tests/host
python3 scripts/device/session.py --help
```

设备操作工具需要 pyserial / esptool；素材脚本需要 Node.js 和 Sharp。统一检查不连接
设备。具体构建与交付命令见 [开发指南](../docs/guides/development.md)。
