# 项目结构说明

## 📁 新的目录结构

```
esp-32/
├── 📄 README.md                    # 项目入口文档
├── 📄 QUICKSTART.md                # 快速启动指南
├── 📄 Cargo.toml                   # Rust 项目配置
├── 📄 build.rs                     # Slint 编译脚本
│
├── 📂 .cargo/                      # Cargo 配置
│   ├── config.toml                # 桌面开发配置 ⭐ 当前使用
│   └── config.esp32.toml          # ESP32 真机配置（切换时重命名）
│
├── 📂 docs/                        # 文档和设计
│   ├── PROJECT_SUMMARY.md         # 项目总结
│   ├── DEVELOPMENT.md             # 开发指南
│   └── eve-fan-control-*.html     # 设计演示（10+ 个版本）
│
└── 📂 src/                         # 源代码 ⭐⭐⭐
    ├── 📄 main.rs                 # 主入口（桌面开发版）
    │
    ├── 📂 service/                # 硬件交互层（桌面开发版）
    │   ├── mod.rs                # 模块定义
    │   ├── hardware.rs           # HAL trait 定义 ⭐
    │   ├── mock.rs               # 模拟硬件（开发用）⭐
    │   ├── esp32.rs              # 真机硬件（待实现）
    │   └── animation.rs          # 动画控制
    │
    ├── 📂 ui/                     # UI 层
    │   ├── mod.rs                # UI 模块
    │   └── fan_control.slint     # Slint UI 定义 ⭐
    │
    └── 📂 embedded_hal/           # ESP32 真机实现（no_std）
        ├── README.md             # 硬件实现说明
        ├── board.rs              # 引脚映射
        ├── display.rs            # AMOLED 驱动
        ├── fan.rs                # PWM 控制
        └── gui.rs                # Slint 集成
```

---

## 🔀 两个版本说明

项目包含两个并行的实现版本：

### 桌面开发版（当前活动）
- **位置**: `src/main.rs` + `src/service/` + `src/ui/`
- **特点**: 使用 `std` 库，Mock 硬件，用于 UI 开发
- **运行**: `cargo run`

### ESP32 真机版（等待硬件）
- **位置**: `src/embedded_hal/`
- **特点**: `no_std` + `esp-hal`，真实硬件驱动
- **说明**: 详见 `src/embedded_hal/README.md`

---

## 🎯 分层架构

### 第 1 层：主入口 (`src/main.rs`)

**职责**:
- 创建 UI 实例
- 初始化硬件（模拟或真机）
- 绑定回调函数
- 启动数据更新循环

**关键代码**:
```rust
fn main() {
    let ui = FanControlUI::new()?;
    let state = FanControlState::new_mock();  // 或 new_hardware()
    
    // 绑定回调
    ui.on_speed_changed(|speed| { ... });
    
    // 启动更新循环
    std::thread::spawn(|| { ... });
    
    ui.run()
}
```

### 第 2 层：服务层 (`src/service/`)

#### `hardware.rs` - 硬件抽象层 ⭐

**定义 3 个 trait**:
- `TemperatureSensor` - 温度传感器接口
- `FanController` - 风扇控制接口
- `CpuTempSensor` - CPU 温度接口

**状态管理**:
- `FanControlState` - 应用状态管理
  - `new_mock()` - 创建模拟硬件
  - `new_hardware()` - 创建真机硬件
  - `update()` - 更新传感器数据
  - `set_fan_speed()` - 设置风扇档位

#### `mock.rs` - 模拟硬件实现 ⭐

**用途**: 桌面开发，无需真实硬件

**实现**:
- `MockTempSensor` - 模拟温度 ± 波动
- `MockFanController` - 模拟 PWM 和转速
- `MockCpuTempSensor` - 模拟 CPU 温度

#### `esp32.rs` - 真机硬件（待实现）

**TODO**: 硬件到货后实现
- `Esp32TempSensor` - I2C 温度传感器
- `Esp32FanController` - LEDC PWM 控制
- `Esp32CpuTempSensor` - ESP32 内部温度

#### `animation.rs` - 动画控制

**TODO**: CRT 上电动画等

### 第 3 层：UI 层 (`src/ui/`)

#### `fan_control.slint` - UI 定义 ⭐

**组件**:
- `eve-colors` - 颜色系统
- `EvePanel` - EVE 风格面板
- `HexButton` - 六边形按钮
- `TempDisplay` - 温度显示
- `RpmDisplay` - 转速显示
- `FanControlUI` - 主界面

**数据绑定**:
```rust
ui.set_temperature(28.5);
ui.set_fan_rpm(2450);
ui.on_speed_changed(|speed| { ... });
```

---

## 🔄 开发 vs 真机切换

### 桌面开发（当前模式）

**配置**: `.cargo/config.toml` (已激活)

**特点**:
- ✅ 使用模拟硬件
- ✅ 在 macOS/Linux/Windows 运行
- ✅ 快速迭代开发

**运行**:
```bash
cargo run
```

### ESP32 真机编译

**配置**: 需要切换配置文件

**步骤**:
```bash
# 1. 切换配置
rm .cargo/config.toml
mv .cargo/config.esp32.toml .cargo/config.toml

# 2. 取消注释 Cargo.toml 中的 esp-idf-* 依赖

# 3. 实现 src/service/esp32.rs 中的硬件驱动

# 4. 修改 src/main.rs，使用 new_hardware()

# 5. 编译并烧录
cargo build --release
espflash flash target/riscv32imafc-unknown-none-elf/release/fan-control
```

---

## 🛠️ 修改指南

### 修改 UI

**文件**: `src/ui/fan_control.slint`

**示例**: 修改颜色
```slint
export global eve-colors {
    out property <color> cyan-primary: #5fd0e0;  // 修改这里
    // ...
}
```

### 添加硬件功能

**文件**: `src/service/hardware.rs`

**示例**: 添加新 trait
```rust
pub trait NewSensor: Send {
    fn read_data(&self) -> Result<Data, &'static str>;
}
```

### 实现真机驱动

**文件**: `src/service/esp32.rs`

**示例**: 实现温度传感器
```rust
pub struct Esp32TempSensor {
    i2c: esp_idf_hal::i2c::Master<...>,
}

impl TemperatureSensor for Esp32TempSensor {
    fn read_celsius(&self) -> Result<f32, &'static str> {
        // 实现 I2C 读取
    }
}
```

### 修改主逻辑

**文件**: `src/main.rs`

**示例**: 修改更新频率
```rust
std::thread::spawn(move || {
    loop {
        std::thread::sleep(Duration::from_millis(500));  // 改为 500ms
        // ...
    }
});
```

---

## 📊 文件职责总结

| 文件 | 职责 | 修改频率 |
|------|------|---------|
| `src/main.rs` | 程序入口、初始化 | 低 |
| `src/service/hardware.rs` | HAL trait 定义 | 低 |
| `src/service/mock.rs` | 模拟硬件 | 低 |
| `src/service/esp32.rs` | 真机硬件 | 高（待实现）|
| `src/ui/fan_control.slint` | UI 定义 | 中 |
| `.cargo/config.toml` | 编译配置 | 低（切换时）|

---

## 🎯 下一步开发

### Phase 1: 完善 UI（无需硬件）

- [ ] 实现圆环进度条（Slint Path）
- [ ] 添加辉光滤镜效果
- [ ] 完善 CRT 上电动画

**文件**: `src/ui/fan_control.slint`

### Phase 2: 实现硬件驱动（硬件到货后）

- [ ] I2C 温度传感器
- [ ] LEDC PWM 风扇控制
- [ ] GPIO 转速读取

**文件**: `src/service/esp32.rs`

### Phase 3: 集成测试

- [ ] 真机烧录
- [ ] 数据验证
- [ ] 性能测试

**文件**: 全部

---

## 💡 关键设计理念

### 1. 分层清晰

```
UI 层 (fan_control.slint)
    ↓
入口层 (main.rs)
    ↓
服务层 (service/)
    ↓
硬件层 (hardware.rs traits)
```

### 2. 抽象隔离

- **HAL Trait** 隔离硬件差异
- **模拟实现** 支持无硬件开发
- **真机实现** 即插即用

### 3. 模块独立

- `service/` 可以独立测试
- `ui/` 可以独立开发
- `main.rs` 只负责组装

---

## 📝 常见任务

### 添加新传感器

1. 在 `hardware.rs` 定义 trait
2. 在 `mock.rs` 实现模拟版本
3. 在 `esp32.rs` 实现真机版本
4. 在 `main.rs` 集成

### 修改 UI 布局

1. 打开 `src/ui/fan_control.slint`
2. 修改组件布局
3. `cargo run` 测试

### 切换到真机

1. 切换 `.cargo/config.toml`
2. 实现 `esp32.rs` 驱动
3. 修改 `main.rs` 使用 `new_hardware()`
4. 编译烧录

---

**项目结构清晰，易于维护和扩展！** ✨
