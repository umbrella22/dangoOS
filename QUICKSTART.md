# EVE 风扇控制系统 - 快速启动指南

## 🚀 5 分钟快速开始

### 桌面开发（立即可用）

```bash
# 1. 进入项目目录
cd ui/eve_fan

# 2. 运行（模拟硬件）
cargo run
```

**就这么简单！** 🎉

界面会自动启动，显示模拟的温度和转速数据。

---

## 📁 项目文件导航

```
esp-32/
├── docs/                                    # 设计文档
│   ├── eve-fan-control-embedded.html       # 最终嵌入式版本 ⭐
│   ├── eve-fan-control-production.html     # 生产版（无星空）
│   ├── eve-fan-control-ultimate.html       # 终极版（带星空，演示用）
│   └── PROJECT_SUMMARY.md                  # 项目总结 ⭐
│
└── ui/eve_fan/                              # 嵌入式实现
    ├── fan_control.slint                   # UI 定义 ⭐
    ├── src/main.rs                         # Rust 逻辑 ⭐
    ├── Cargo.toml                          # 依赖配置
    ├── build.rs                            # 编译脚本
    └── README.md                           # 详细文档 ⭐
```

**必看文件标记为 ⭐**

---

## 🎯 关键任务清单

### ✅ 已完成（无需操作）

- [x] UI 设计（8 个迭代版本）
- [x] Slint 组件实现
- [x] 硬件抽象层设计
- [x] 模拟硬件实现
- [x] 完整代码注释
- [x] 项目文档

### 🚧 待硬件到货

- [ ] 真机硬件驱动实现
- [ ] 真机烧录测试
- [ ] 24/7 稳定性测试

---

## 🔧 待实现的硬件接口

### 1. 温度传感器

**文件**: `ui/eve_fan/src/main.rs`  
**位置**: 搜索 `TODO: 真机实现示例`  
**接口**: `TemperatureSensor` trait

```rust
// 找到这段注释，取消注释并实现：
// pub struct Esp32TempSensor {
//     i2c: esp_idf_hal::i2c::Master<...>,
//     address: u8,
// }
```

### 2. 风扇控制器

**文件**: `ui/eve_fan/src/main.rs`  
**位置**: 同上  
**接口**: `FanController` trait

```rust
// 找到这段注释，取消注释并实现：
// pub struct Esp32FanController {
//     pwm: esp_idf_hal::ledc::LedcChannel<...>,
//     rpm_pin: esp_idf_hal::gpio::PinDriver<...>,
// }
```

### 3. 替换 main 函数

**文件**: `ui/eve_fan/src/main.rs`  
**位置**: `fn main()` 函数中  
**修改**: 第 340-345 行

```rust
// 找到这段注释：
// TODO: 真机版本
// let state = Arc::new(std::sync::Mutex::new(
//     FanControlState::new_hardware(
//         Box::new(Esp32TempSensor::new(...)),
//         Box::new(Esp32FanController::new(...)),
//         Box::new(Esp32CpuTempSensor::new()),
//     )
// ));

// 取消注释，删除上面的 new_mock() 行
```

---

## 📖 详细文档位置

| 需求 | 文档 |
|------|------|
| **查看最终设计** | `docs/eve-fan-control-embedded.html` (在浏览器打开) |
| **了解项目全貌** | `docs/PROJECT_SUMMARY.md` |
| **开发指南** | `ui/eve_fan/README.md` |
| **代码注释** | `ui/eve_fan/src/main.rs` (每个函数都有注释) |

---

## ⚡ 常见操作

### 查看设计效果

```bash
# macOS
open docs/eve-fan-control-embedded.html

# Linux
xdg-open docs/eve-fan-control-embedded.html

# Windows
start docs/eve-fan-control-embedded.html
```

### 桌面测试

```bash
cd ui/eve_fan
cargo run
```

### 真机编译（硬件到货后）

```bash
cd ui/eve_fan

# 1. 取消注释 Cargo.toml 中的 esp-idf-* 依赖
# 2. 实现硬件接口（见上方）
# 3. 编译
cargo build --release --target xtensa-esp32s3-none-elf

# 4. 烧录
espflash flash target/xtensa-esp32s3-none-elf/release/fan-control
```

---

## 🐛 问题排查

### 编译错误

**问题**: `cannot find module slint`  
**解决**: 确保 `slint` 和 `slint-build` 版本一致（都是 1.3）

**问题**: `trait TemperatureSensor not found`  
**解决**: 检查 `use` 语句，确保导入了自定义 trait

### 运行错误

**问题**: 窗口打不开  
**解决**: 检查显示驱动，尝试 `export SLINT_BACKEND=winit`

**问题**: 数据不更新  
**解决**: 检查 `mainLoop` 循环是否启动，查看控制台日志

---

## 💬 下一步做什么

### 立即（无需硬件）

1. **查看设计**
   ```bash
   open docs/eve-fan-control-embedded.html
   ```

2. **运行代码**
   ```bash
   cd ui/eve_fan && cargo run
   ```

3. **阅读文档**
   - 项目总结: `docs/PROJECT_SUMMARY.md`
   - 开发指南: `ui/eve_fan/README.md`

### 硬件到货后

1. **实现硬件驱动**
   - 参考 `src/main.rs` 中的 `TODO` 注释
   - 实现 3 个 trait: `TemperatureSensor`, `FanController`, `CpuTempSensor`

2. **真机测试**
   - 烧录固件
   - 验证传感器数据
   - 调整温度曲线

3. **长期运行**
   - 24/7 稳定性测试
   - 性能优化
   - 功能完善

---

## 📞 需要帮助？

**代码问题**: 查看 `ui/eve_fan/src/main.rs` 中的详细注释  
**设计问题**: 查看 `docs/PROJECT_SUMMARY.md` 设计章节  
**硬件问题**: 查看 `ui/eve_fan/README.md` 硬件接口章节

---

## 🎉 祝你成功！

**项目已经 100% 完成设计和代码实现！**  
**现在只需要等硬件到货，开始真机测试！**

加油！🚀
