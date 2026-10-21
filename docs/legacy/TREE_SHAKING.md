# Feature 与编译边界

workspace 使用 Cargo resolver 2。桌面和 MCU 分别构建，公共依赖在根 `Cargo.toml` 声明，平台能力由消费 crate 选择。

## UI feature

| `fan-ui` feature | 行为 |
| --- | --- |
| `std`（默认） | 使用 Slint 的标准库支持 |
| 未启用 `std` | `no_std` 固件形态 |
| `embedded` | 启用软件渲染、libm 和单线程支持；构建时嵌入软件渲染资源 |

workspace 的 `fan-ui` 依赖关闭默认 feature。`fan-desktop` 显式启用 `std`，并启用 Slint 桌面后端；`fan-firmware` 显式启用 `embedded`。`fan-ui` 本身不选择桌面窗口后端。

`build.rs` 每次只编译所选布局。资源位于 UI crate 内，字体导入不跨越其他 crate 的源码目录。

## 应用目标

| 命令 | 应用 | 布局 |
| --- | --- | --- |
| `cargo desktop` | 主机桌面程序 | 480×480 |
| `cargo firmware` | S31 `no_std` 固件（ESP-Mosaico v1.2） | 480×480 |

目标板只有 ESP-Mosaico 一种；历史上的 Korvo-1 双变体已随其硬件迭代移除。

## 验证

```bash
./scripts/check.sh
cargo tree -p fan-firmware --target riscv32imafc-unknown-none-elf --edges normal
```

固件运行时依赖不包含 `fan-desktop`、桌面窗口后端或 `esp-idf-hal/sys/svc`。Slint 构建脚本在主机运行，其编译工具可以使用标准库；根 `Cargo.lock` 出现桌面依赖也不表示它们进入了固件。

release 使用 `opt-level=3`、fat LTO 和单个 codegen unit。具体镜像大小由实际产物衡量，不能从 feature 名称推断固定节省比例。
