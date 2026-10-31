# 工程测试

| 位置 | 执行方式 | 范围 |
| --- | --- | --- |
| `tests/host` | `python3 -B -m unittest discover -s tests/host` | 实际 Python 工具与模拟 USB、NOR 分区行为，不连接设备 |
| `crates/*/src` | `cargo test --workspace --offline` | 各组件的 Rust 单元测试 |
| `crates/os/tests` | 同上，或 `cargo test -p os --test photo --offline` | 主机 LVGL、指针、渲染边界与 Photo 交互 |
| `tools/app-pack/src/tests.rs` | `cargo test -p app-pack --offline` | 完整应用集、system 镜像、更新覆盖与卸载回退 |

`tests/host/support.py` 提供共享的 USB 模拟与 CLI 加载辅助。
引擎的图像、缩放、线程、显示和硬件测试资料统一放在 [crates/lua-engine/test](../crates/lua-engine/test/README.md)，其中 `reference/` 保存仅供接口对照的上游资料。
正式应用与开发示例分别位于 `apps` 和 `examples/apps`。

统一离线检查入口为 `scripts/check.sh`。实体设备验收使用正式固件、正式应用和
`scripts/device/session.py` 的有时限会话；构建或主机回归通过与实体操作结果分别记录。
