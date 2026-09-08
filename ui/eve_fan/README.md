# EVE 风格风扇控制系统 - 嵌入式实现

## 📋 项目概述

基于 Slint UI 框架的 EVE Online 风格风扇控制界面，专为 ESP32-S31 + AMOLED 显示屏设计。

**设计分辨率**: 480×480 像素  
**目标平台**: ESP32-S31 (ESP-IDF)  
**UI 框架**: Slint 1.3  
**渲染方式**: 软件渲染 (30fps)

---

## 🎨 视觉效果

- ✅ EVE Online 青金石蓝配色风格
- ✅ 分层 CRT 上电动画（边框先闪，内容后闪）
- ✅ 六边形档位控制（0/25/50/75/100/AUTO）
- ✅ 实时温度/转速监控
- ✅ 圆环百分比显示
- ✅ 无星空背景（性能优化）

---

## 📁 项目结构

```
ui/eve_fan/
├── fan_control.slint       # Slint UI 定义
├── src/
│   └── main.rs             # Rust 后端逻辑
├── build.rs                # Slint 编译脚本
├── Cargo.toml              # 依赖配置
└── README.md               # 本文档
```

---

## 🔧 开发环境搭建

### 1. 桌面开发（模拟硬件）

**前置要求**:
- Rust 1.70+
- Slint 1.3+

**编译运行**:
```bash
cd ui/eve_fan
cargo run
```

**特性**:
- 使用模拟传感器和风扇控制器
- 可在 Windows/macOS/Linux 上运行
- 适合 UI 开发和逻辑调试

### 2. ESP32 真机编译

**前置要求**:
- ESP-IDF 5.0+
- Rust ESP 工具链
- espflash

**编译**:
```bash
# 安装 ESP Rust 工具链
rustup target add xtensa-esp32s3-none-elf

# 编译
cd ui/eve_fan
cargo build --release --target xtensa-esp32s3-none-elf

# 烧录
espflash flash target/xtensa-esp32s3-none-elf/release/fan-control
```

**注意**: 需要取消注释 `Cargo.toml` 中的 `esp-idf-*` 依赖。

---

## 🛠️ 硬件接口实现

### 当前状态：模拟硬件

`src/main.rs` 中默认使用模拟硬件：
- `MockTempSensor` - 模拟温度传感器
- `MockFanController` - 模拟 PWM 风扇
- `MockCpuTempSensor` - 模拟 CPU 温度

### 待实现：真机硬件

**1. 温度传感器接口**

```rust
pub trait TemperatureSensor {
    fn read_celsius(&self) -> Result<f32, &'static str>;
}
```

**实现建议**:
- I2C 传感器（如 DS18B20, DHT22）
- SPI 传感器
- 内置 ADC + NTC 热敏电阻

**示例实现**（`src/main.rs` 中已注释）:
```rust
pub struct Esp32TempSensor {
    i2c: esp_idf_hal::i2c::Master<...>,
    address: u8,
}

impl TemperatureSensor for Esp32TempSensor {
    fn read_celsius(&self) -> Result<f32, &'static str> {
        // 从 I2C 读取传感器数据
        // ...
    }
}
```

**2. 风扇控制接口**

```rust
pub trait FanController {
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str>;
    fn read_rpm(&self) -> Result<u16, &'static str>;
}
```

**实现建议**:
- PWM 输出：ESP32 LEDC 通道
- 转速反馈：GPIO 中断计数 TACH 脉冲

**示例实现**（`src/main.rs` 中已注释）:
```rust
pub struct Esp32FanController {
    pwm: esp_idf_hal::ledc::LedcChannel<...>,
    rpm_pin: esp_idf_hal::gpio::PinDriver<...>,
}

impl FanController for Esp32FanController {
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str> {
        let duty = (percentage as u32 * self.pwm.get_max_duty() / 100) as u32;
        self.pwm.set_duty(duty)
            .map_err(|_| "PWM set duty error")
    }
    
    fn read_rpm(&self) -> Result<u16, &'static str> {
        // 从 TACH 引脚读取转速脉冲
        // ...
    }
}
```

**3. CPU 温度接口**

```rust
pub trait CpuTempSensor {
    fn read_celsius(&self) -> Result<i32, &'static str>;
}
```

**实现建议**:
- ESP32 内部温度传感器
- 外部监控芯片

---

## 📊 数据流架构

```
┌─────────────┐
│ 硬件传感器   │
│ - 温度传感器 │
│ - 风扇 TACH │
│ - CPU 温度  │
└──────┬──────┘
       │ read()
       ↓
┌─────────────────┐
│ FanControlState │  ← 每秒更新
│ - 读取传感器    │
│ - 计算趋势     │
│ - 自动控制     │
└──────┬──────────┘
       │ update()
       ↓
┌─────────────────┐
│  Slint UI      │
│ - 显示数据     │
│ - 用户交互     │
└──────┬──────────┘
       │ speed_changed()
       ↓
┌─────────────────┐
│ PWM 控制器     │
│ - 设置档位     │
└─────────────────┘
```

---

## 🎯 功能清单

### ✅ 已实现

- [x] Slint UI 完整布局
- [x] 模拟传感器和风扇
- [x] 档位切换逻辑
- [x] AUTO 模式（温度自动调节）
- [x] 温度趋势计算
- [x] 功率估算
- [x] 运行时间格式化
- [x] 硬件接口 trait 定义

### 🚧 待实现

- [ ] CRT 上电动画集成
- [ ] 圆环进度条绘制（Slint Path）
- [ ] 辉光滤镜效果
- [ ] 真机硬件驱动
- [ ] I2C 温度传感器
- [ ] PWM 风扇控制
- [ ] TACH 转速读取
- [ ] 看门狗集成
- [ ] 错误恢复机制
- [ ] 日期时间同步

---

## 🚀 开发路线图

### Phase 1: UI 完善（当前阶段）

**目标**: 在桌面环境完成 UI 开发
- [x] 创建 Slint 组件
- [x] 实现模拟硬件
- [x] 数据绑定和更新
- [ ] 完善动画效果
- [ ] 性能优化

**测试**: 桌面环境 `cargo run`

### Phase 2: 硬件驱动

**目标**: 实现真机硬件接口
- [ ] I2C 温度传感器驱动
- [ ] LEDC PWM 输出
- [ ] GPIO 转速读取
- [ ] CPU 温度读取

**测试**: 真机烧录，硬件回环测试

### Phase 3: 集成测试

**目标**: 完整功能验证
- [ ] 24/7 稳定性测试
- [ ] 温度控制精度测试
- [ ] 性能和功耗测试
- [ ] 错误恢复测试

**测试**: 长期运行，极端环境

### Phase 4: 生产优化

**目标**: 生产环境部署
- [ ] 看门狗集成
- [ ] OTA 更新支持
- [ ] 日志和监控
- [ ] 配置文件支持

---

## ⚡ 性能目标

| 指标 | 目标 | 实测 | 状态 |
|------|------|------|------|
| 帧率 | 30fps | - | 待测 |
| CPU 占用 | < 50% | - | 待测 |
| 内存占用 | < 500KB | - | 待测 |
| 启动时间 | < 3s | - | 待测 |
| 功耗 | < 500mW | - | 待测 |

---

## 🐛 已知问题

1. **Slint 圆环进度条**: 需要用 `Path` 手动绘制，SVG 中的 `stroke-dasharray` 无法直接移植
2. **辉光滤镜**: Slint 不支持 SVG `filter`，需要用 `drop-shadow` 或其他方式模拟
3. **CRT 动画**: 需要手动管理动画状态，Slint 的 `states` 机制有限制

---

## 📝 开发日志

### 2024-12-XX

- ✅ 创建 Slint UI 定义
- ✅ 实现 Rust 后端框架
- ✅ 设计硬件抽象层
- ✅ 实现模拟硬件用于开发

---

## 🤝 贡献指南

1. **UI 开发**: 修改 `fan_control.slint`
2. **逻辑开发**: 修改 `src/main.rs`
3. **硬件驱动**: 实现 `TemperatureSensor`/`FanController` trait
4. **测试**: 添加单元测试和集成测试

---

## 📄 许可证

MIT License

---

## 👥 作者

- **Kiro** - 硬件工程师
- **Claude** - AI 助手

---

## 🔗 相关资源

- [Slint 官方文档](https://slint.dev/docs)
- [ESP-IDF 文档](https://docs.espressif.com/projects/esp-idf/)
- [EVE Online UI 参考](https://www.eveonline.com/)

---

**准备就绪！等待硬件到货后进行真机测试！** 🚀
