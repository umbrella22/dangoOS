# lua-engine：Lua 运行时与宿主接口

提供 Lua 5.5.1 VM、执行预算、协作式调度、后台 worker，以及显示、存储和硬件模块。
Rust 宿主通过 `Host`、`Request` 和硬件快照接入设备能力；主机测试使用相同接口提供模拟数据。

| 目录 | 内容 | 正式固件构建 |
| --- | --- | --- |
| `src/` | Rust 运行时与 `#[cfg(test)]` 单元测试模块 | 编译运行时代码；测试模块只在 `cargo test` 中编译 |
| `csrc/` | 项目自有的 C 桥接、平台适配和内置 Lua 模块 | C 代码参与编译，内置 Lua 模块由 `build.rs` 嵌入 |
| `vendor/` | 运行期第三方源码、字体、配套说明和许可证 | `build.rs` 明确列出的源码、头文件和内嵌脚本参与构建 |
| [`test/`](test/README.md) | 图像、缩放、线程、显示与硬件测试资料，以及上游接口参考 | 不编入固件，不写入 system 应用镜像 |

`test/` 的名称是目录约定。构建边界由 `src/lib.rs` 和 `src/jpeg.rs` 的 `#[cfg(test)]`
以及 `build.rs` 的显式输入列表保证；主机构建测试程序时才读取测试脚本、图像和 C 对照程序。
`test/reference/` 中的文件只提供接口对照，不由自动测试执行。

`vendor/` 中的来源、许可证及本地修改见 [NOTICE](vendor/NOTICE.md)。
正式 Lua 应用位于仓库 `apps/`，由 `app-pack` 单独构建 system 镜像。

从仓库根目录运行：

```sh
cargo test -p lua-engine --offline
cargo test -p os --test photo --offline
./scripts/check.sh
```
