# 工程测试

| 位置 | 执行方式 | 范围 |
| --- | --- | --- |
| `tests/host` | `python3 -B -m unittest discover -s tests/host` | 实际 Python 工具与模拟 USB、NOR 分区行为，不连接设备 |
| `tests/device/apps` | 通过 `scripts/device/session.py` 推送指定探针 | 需要设备、操作动作和观察记录的硬件验收 |
| `crates/*/src` | `cargo test --workspace --offline` | 各组件的 Rust 单元测试 |
| `crates/os/tests` | 同上，或 `cargo test -p os --test photo --offline` | 主机 LVGL、指针、渲染边界与 Photo 交互 |
| `tools/app-pack/src/tests.rs` | `cargo test -p app-pack --offline` | 完整应用集、system 镜像、更新覆盖与卸载回退 |

`tests/host/support.py` 提供共享的 USB 模拟与 CLI 加载辅助。
引擎的图像、缩放、线程、显示和硬件测试资料统一放在 [crates/lua-engine/test](../crates/lua-engine/test/README.md)，其中 `reference/` 保存仅供接口对照的上游资料。
正式应用、开发示例与设备探针分别位于 `apps`、`examples/apps`、`tests/device/apps`。

统一离线检查入口为 `scripts/check.sh`。设备探针的完成日志与真实硬件验收分别记录，
运行方式及 PASS / SKIP / 预期异常含义见 [设备测试](device/README.md)。
