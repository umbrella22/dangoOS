# 工程测试

| 位置 | 执行方式 | 范围 |
| --- | --- | --- |
| `tests/host` | `python3 -B -m unittest discover -s tests/host` | 实际 Python 工具与模拟 USB、NOR 分区行为，不连接设备 |
| `firmware/**/src` | `cargo test --workspace --offline` | 各组件的 Rust 单元测试 |
| `firmware/os/tests` | 同上，或 `cargo test -p os --test photo --offline` | 主机 LVGL、指针、渲染边界与 Photo 交互 |
| `tools/app-pack/src/tests.rs` | `cargo test -p app-pack --offline` | 完整应用集、system 镜像、更新覆盖与卸载回退 |
| `firmware/os/tests/ts_app.rs` | `cargo test -p os --no-default-features --features ts-engine --test ts_app --offline` | 预编译 EmbedTS、真实 LVGL/LittleFS、模拟板级服务和资源释放 |
| `firmware/os/tests/ts_graphics.rs` | 同上，将 test 改为 `ts_graphics` | RGB565、DFN1、PNG/JPEG、句柄、校准、按钮和资源配额 |
| `firmware/os/tests/ts_official.rs` | 同上，将 test 改为 `ts_official` | 四个游戏规则/绘制与相册浏览、确认删除 |
| 外部 EmbedTypescript `tests/device/ts-parity` | 见[探针说明](https://github.com/umbrella22/EmbedTypescript/blob/main/tests/device/ts-parity/README.md) | 实体设备 SDK、worker、显示、输入和常驻事务 |
| `tests/benchmarks/script-engines` | 见目录 README | Lua / TS 成对计算负载、release 主机跑分和设备日志比较 |
| `tools/check/engines.sh` | 同左 | 预编译输入、TS OS 与打包回归、互斥 features 和两种固件隔离 |

`tests/host/support.py` 提供共享的 USB 模拟与 CLI 加载辅助。
引擎的图像、缩放、线程、显示和硬件测试资料统一放在 [firmware/runtime/lua-engine/test](../firmware/runtime/lua-engine/test/README.md)，其中 `reference/` 保存仅供接口对照的上游资料。
正式应用位于 `apps/builtin`；可直接安装的 Lua 示例位于 `apps/examples`，需要先构建的
TS 源码、构建器测试、设备探针和基准在独立 EmbedTypescript 仓库。OS 集成测试读取 `resources/embedts` 预编译夹具。

统一离线检查入口为 `tools/check.sh`。实体设备验收使用正式固件、正式应用和
`tools/device/session.py` 的有时限会话；构建或主机回归通过与实体操作结果分别记录。
