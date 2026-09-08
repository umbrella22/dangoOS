# ESP32 嵌入式硬件抽象层 (Embedded HAL)

这个目录包含针对 ESP-Mosaico 开发板的真实硬件实现代码。

## 文件说明

- **`board.rs`** - ESP-Mosaico 开发板引脚映射和硬件常量
- **`display.rs`** - CO5300 QSPI AMOLED 显示驱动 (480×480)
- **`fan.rs`** - 风扇控制任务 (PWM 输出，事件驱动)
- **`gui.rs`** - Slint ↔ esp-rtos 集成层 (软件渲染器)

## 编译目标

这些文件是 **`no_std`** 嵌入式 Rust 代码，需要切换到 ESP32 编译配置：

```bash
# 切换到 ESP32 配置
cp .cargo/config.esp32.toml .cargo/config.toml

# 编译并烧录
cargo build --release
espflash flash --monitor target/riscv32imafc-unknown-none-elf/release/fan-control
```

## 与桌面版的关系

- **桌面开发版本**: `src/main.rs` + `src/service/` + `src/ui/` (当前活动版本)
  - 使用 `std` 库
  - Mock 硬件实现
  - 用于 UI 开发和预览

- **ESP32 真机版本**: 这个目录的代码
  - 使用 `no_std` + `esp-hal`
  - 真实硬件驱动
  - 需要物理硬件

## 硬件依赖

详见 `Cargo.toml` 中注释掉的 ESP32 依赖：

```toml
# esp-idf-hal = "0.42"
# esp-idf-sys = "0.33"
# embedded-hal = "0.2"
```

## 项目状态

✅ 代码已完成架构优化 (移除 static mut, 错误恢复, 看门狗)  
⏳ 等待硬件到货进行真机测试

---

**注意**: 这些文件不被当前的桌面开发版本 `src/main.rs` 引用，它们是独立的嵌入式实现。
