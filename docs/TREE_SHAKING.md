# 树摇优化（Tree Shaking）实现

本项目通过 **Rust 条件编译** 实现树摇优化，确保不同硬件变体的固件只包含各自需要的代码。

## 工作原理

### 1. 编译时选择 UI 文件

**`build.rs`** 根据 feature 选择不同的 Slint UI 文件：

```rust
#[cfg(feature = "s31-variant-a")]
let ui_file = "src/ui/fan_control.slint";        // 480×480 方屏

#[cfg(feature = "s31-variant-b")]
let ui_file = "src/ui/fan_control_800x480.slint"; // 800×480 宽屏

slint_build::compile(ui_file).unwrap();
```

**结果**：编译后的固件只包含目标硬件的 UI 代码，不会包含其他变体的 UI。

### 2. 引脚映射树摇

**`src/embedded_hal/board.rs`** 使用条件编译导出不同的引脚定义：

```rust
// 变体 A 的引脚
#[cfg(feature = "s31-variant-a")]
pub mod variant_a {
    pub const LCD_SCLK_GPIO: u8 = 44;
    pub const LCD_TYPE: &str = "QSPI-AMOLED";
    // ... 更多引脚
}

// 变体 B 的引脚
#[cfg(feature = "s31-variant-b")]
pub mod variant_b {
    pub const LCD_PCLK_GPIO: u8 = 42;
    pub const LCD_TYPE: &str = "RGB-LCD";
    // ... 不同的引脚布局
}

// 根据 feature 导出
#[cfg(feature = "s31-variant-a")]
pub use variant_a::*;

#[cfg(feature = "s31-variant-b")]
pub use variant_b::*;
```

**结果**：编译时只保留目标变体的引脚常量，未使用的变体模块会被完全移除。

### 3. 硬件特性树摇

**特性标识**允许根据硬件能力条件编译代码：

```rust
// board.rs
#[cfg(feature = "s31-variant-a")]
pub const HAS_RGB_LED: bool = false;
pub const HAS_AUDIO: bool = false;

#[cfg(feature = "s31-variant-b")]
pub const HAS_RGB_LED: bool = true;
pub const HAS_AUDIO: bool = true;
```

**在驱动代码中使用**：

```rust
// 只有 Korvo-1 才会编译 RGB LED 驱动
#[cfg(all(feature = "s31-variant-b", board::HAS_RGB_LED))]
mod rgb_led {
    pub fn init() { /* WS2812 初始化 */ }
    pub fn set_color(r: u8, g: u8, b: u8) { /* ... */ }
}

// Mosaico 编译时，整个 rgb_led 模块不会被包含
```

### 4. 帧缓冲大小优化

不同屏幕尺寸使用不同的缓冲区大小：

```rust
#[cfg(feature = "s31-variant-a")]
pub const LCD_FB_BYTES: usize = 480 * 480 * 2;  // 450KB

#[cfg(feature = "s31-variant-b")]
pub const LCD_FB_BYTES: usize = 800 * 480 * 2;  // 750KB
```

**结果**：Mosaico 固件只分配 450KB 帧缓冲，Korvo-1 分配 750KB，不会浪费内存。

## 验证树摇效果

### 编译不同变体

```bash
# 编译 Mosaico (variant-a)
cargo build --release --features s31-variant-a

# 编译 Korvo-1 (variant-b)
cargo build --release --features s31-variant-b
```

### 检查固件大小

```bash
# 变体 A 固件
ls -lh target/riscv32imafc-unknown-none-elf/release/fan-control

# 变体 B 固件
ls -lh target/riscv32imafc-unknown-none-elf/release/fan-control
```

### 检查符号表

查看编译后的固件包含哪些符号：

```bash
# 查看 variant-a 的符号
riscv32-esp-elf-nm target/.../release/fan-control | grep -i "variant"

# 应该只看到 variant_a 相关的符号，没有 variant_b
```

### 反汇编验证

```bash
# 反汇编固件
riscv32-esp-elf-objdump -d target/.../release/fan-control > disasm.txt

# 搜索未使用的变体符号
grep "variant_b" disasm.txt  # 在 variant-a 固件中应该找不到
```

## 树摇效果对比

### 理论节省

| 组件 | Variant A | Variant B | 共享 |
|------|-----------|-----------|------|
| UI 代码 | 480×480 布局 | 800×480 布局 | ❌ |
| 引脚定义 | QSPI 引脚 | RGB 引脚 | ❌ |
| 显示驱动 | CO5300 QSPI | RGB LCD | ❌ |
| 帧缓冲 | 450KB | 750KB | ❌ |
| RGB LED | ❌ 不编译 | ✅ 编译 | ❌ |
| 核心逻辑 | ✅ | ✅ | ✅ |

### 预期固件大小

- **Variant A (Mosaico)**: ~850KB (无音频/RGB LED 驱动)
- **Variant B (Korvo-1)**: ~950KB (包含音频/RGB LED 驱动)
- **代码共享率**: ~70% (核心逻辑、UI 框架、状态管理)

## 添加新的条件编译代码

### 示例 1：硬件特性门控

```rust
// src/embedded_hal/display.rs

// 只为 variant-a 编译 QSPI 驱动
#[cfg(feature = "s31-variant-a")]
pub mod qspi_driver {
    pub fn init_co5300() { /* QSPI AMOLED 初始化 */ }
}

// 只为 variant-b 编译 RGB 驱动
#[cfg(feature = "s31-variant-b")]
pub mod rgb_driver {
    pub fn init_rgb_lcd() { /* RGB LCD 初始化 */ }
}
```

### 示例 2：可选外设

```rust
// src/embedded_hal/peripherals.rs

// 只有 Korvo-1 才编译音频模块
#[cfg(all(
    feature = "s31-variant-b",
    not(feature = "disable-audio")  // 允许运行时禁用
))]
pub mod audio {
    pub fn init_i2s() { /* I2S 音频初始化 */ }
    pub fn play_beep() { /* 播放提示音 */ }
}

// 如果硬件不支持音频，提供空实现
#[cfg(not(all(feature = "s31-variant-b", not(feature = "disable-audio"))))]
pub mod audio {
    pub fn init_i2s() {}
    pub fn play_beep() {}
}
```

### 示例 3：调试代码树摇

```rust
// 只在开发构建中包含调试功能
#[cfg(debug_assertions)]
pub fn print_debug_info() {
    println!("Temperature sensor address: 0x{:x}", SENSOR_ADDR);
    println!("Frame buffer size: {} bytes", LCD_FB_BYTES);
}

#[cfg(not(debug_assertions))]
pub fn print_debug_info() {}  // Release 构建中为空
```

## 最佳实践

### ✅ DO（推荐）

1. **使用 feature gates 隔离硬件差异**
   ```rust
   #[cfg(feature = "s31-variant-a")]
   const PIN: u8 = 44;
   ```

2. **为可选功能提供空实现**
   ```rust
   #[cfg(not(feature = "enable-wifi"))]
   pub fn init_wifi() {}
   ```

3. **在 Cargo.toml 中清晰定义 features**
   ```toml
   [features]
   s31-variant-a = []
   s31-variant-b = ["audio-support", "rgb-led"]
   audio-support = []
   rgb-led = []
   ```

### ❌ DON'T（避免）

1. **运行时检查硬件类型**（浪费内存）
   ```rust
   // ❌ 不好
   if hardware_type == "variant-a" {
       init_qspi();
   } else {
       init_rgb();
   }
   ```

2. **使用全局 if cfg! 宏**（不会树摇）
   ```rust
   // ❌ 不好
   if cfg!(feature = "s31-variant-a") {
       // 这段代码仍然会被编译到两个变体中
   }
   ```

3. **混合变体代码**（破坏树摇）
   ```rust
   // ❌ 不好：两个变体的引脚在同一个结构体中
   pub struct Pins {
       qspi_sclk: u8,  // 只有 A 需要
       rgb_pclk: u8,   // 只有 B 需要
   }
   ```

## 总结

通过条件编译实现的树摇优化：

- ✅ **0 运行时开销**：所有决策在编译时完成
- ✅ **固件更小**：只包含目标硬件需要的代码
- ✅ **类型安全**：编译器检查硬件配置
- ✅ **代码共享**：核心逻辑在变体间复用
- ✅ **可维护**：清晰的 feature 边界

固件构建时，Rust 编译器会自动移除未使用的代码路径，实现真正的"树摇"效果！
