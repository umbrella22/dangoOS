# Lua 引擎测试资料

本目录集中保存主机回归测试的输入和上游接口对照资料。正式固件构建不读取本目录，
system 镜像仅收集仓库 `apps/` 下的应用。Cargo 源码包仍可包含这些文件，以保留测试输入。

| 目录 | 内容 | 自动测试入口 |
| --- | --- | --- |
| [`images/`](images/README.md) | 合成 PNG、JPEG、RGB565 图像，损坏输入和来源记录 | [JPEG 单元测试](../src/jpeg.rs)、[模块测试](../src/tests_modules.rs)和 [Photo 集成测试](../../os/tests/photo.rs) |
| `resize/` | 缩放表达式对照程序 `oracle.c` 和整数优化压力检查 `stress.c` | `src/tests_modules.rs` 中的两个缩放对照测试，运行时使用主机 C 编译器 |
| [`threads/`](threads/README.md) | 上游线程契约脚本与本地 worker 隔离样本 | `src/tests_modules.rs` 中的线程契约、停止和资源释放测试 |
| `display/` | 上游 `display_contract.lua` | `src/tests_modules.rs::display_contract_passes` |
| `hardware/` | IMU、磁力计、按键和灯带的上游脚本及 API 说明 | `src/tests_hardware.rs`，使用模拟板载快照与输出请求 |
| `reference/fuel_gauge/` | 上游电量计 API、外部 I2C 实现和示例 | 不执行；板载电量计通过 `src/tests_hardware.rs` 内的独立脚本测试 |

Rust 测试模块由 `#[cfg(test)]` 控制，使用 `include_str!`、`include_bytes!` 或基于
`CARGO_MANIFEST_DIR` 的路径读取本目录。`test/` 使用单数名称，文件由现有测试入口显式加载。
测试创建的临时文件位于系统临时目录；本目录保留可复用的输入和对照资料。

## 来源与兼容范围

- `display/`、`hardware/` 和 `reference/fuel_gauge/` 中的上游文件来自
  [espressif/esp-claw](https://github.com/espressif/esp-claw/tree/345073de68179542c307e303df81e38baadf77f0)，
  提交为 `345073de68179542c307e303df81e38baadf77f0`，使用
  [Apache-2.0 许可证](../vendor/esp-claw/LICENSE)。Lua 文件保留导入时的内容；API 说明为本地中文译文。
- `threads/` 的上游目录、同一提交的来源信息和许可证在该目录中保存；`hostile_worker.lua` 是本地样本。
- `images/` 的 PNG、JPEG 和 RGB565 样本为合成数据；对应记录保存像素约定和哈希。
- `resize/` 的 C 程序用于比较缩放计算结果，只生成主机可执行文件。

硬件脚本在主机测试中使用模拟接口。灯带脚本的原版 16 灯配置应被板级限制拒绝，
测试再用内存中的 6 灯版本验证适配。电量计的上游外部 I2C 示例仅作参考。
主机测试结果不代表实际外设的电气、时序或显示验收。

## 执行

从仓库根目录运行：

```sh
cargo test -p lua-engine --offline
cargo test -p os --test photo --offline
```

新增测试资料按功能放入对应子目录，并由测试代码显式引用。实际运行所需的 C 或 Lua
模块放在 `csrc/`，运行期第三方依赖放在 `vendor/`，由 `build.rs` 声明构建输入。
