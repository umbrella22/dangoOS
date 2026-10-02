# ESP32-S31 多变体支持

本项目支持两种 ESP32-S31 硬件配置，通过编译时 Features 选择。

## 硬件变体

### Variant A - ESP-Mosaico 官方开发板

- **Feature**: `s31-variant-a`
- **硬件**: 乐鑫官方 ESP-Mosaico 开发套件
- **屏幕**: CO5300 QSPI AMOLED 480×480
- **引脚**: 官方标准映射

### Variant B - 自定义 S31 配置

- **Feature**: `s31-variant-b`
- **硬件**: 自定义 ESP32-S31 配置
- **引脚**: 根据实际硬件在 `board.rs` 中修改

## 编译命令

### 桌面开发（默认）

```bash
cargo run
```

### ESP32-S31 变体 A

```bash
# 开发构建
cargo build --features s31-variant-a

# Release 构建
cargo build --release --features s31-variant-a
```

### ESP32-S31 变体 B

```bash
# 开发构建
cargo build --features s31-variant-b

# Release 构建
cargo build --release --features s31-variant-b
```

## 使用构建脚本（推荐）

项目包含一个便捷的构建脚本 `build-variants.sh`：

```bash
# 查看帮助
./build-variants.sh --help

# 构建变体 A
./build-variants.sh variant-a

# 构建并烧录变体 B（Release）
./build-variants.sh variant-b --release --flash

# 构建、烧录并监视串口（变体 A）
./build-variants.sh variant-a --flash --monitor

# 桌面开发模式
./build-variants.sh desktop
```

## 引脚配置

不同变体的引脚定义在 `src/embedded_hal/board.rs` 中：

```rust
// 变体 A 的引脚
#[cfg(feature = "s31-variant-a")]
pub mod variant_a {
    pub const LCD_SCLK_GPIO: u8 = 44;
    pub const STATUS_LED_GPIO: u8 = 3;
    // ... 更多引脚
}

// 变体 B 的引脚
#[cfg(feature = "s31-variant-b")]
pub mod variant_b {
    pub const LCD_SCLK_GPIO: u8 = 44;
    pub const STATUS_LED_GPIO: u8 = 10;  // 不同的引脚
    // ... 更多引脚
}
```

## 添加新变体

1. **在 `Cargo.toml` 中添加 feature**:

   ```toml
   [features]
   s31-variant-c = []  # 新变体
   ```

2. **在 `board.rs` 中添加配置**:

   ```rust
   #[cfg(feature = "s31-variant-c")]
   pub mod variant_c {
       pub const LCD_SCLK_GPIO: u8 = 40;
       // ... 引脚定义
   }

   #[cfg(feature = "s31-variant-c")]
   pub use variant_c::*;
   ```

3. **更新构建脚本**（可选）

## 条件编译示例

在代码中根据变体执行不同逻辑：

```rust
#[cfg(feature = "s31-variant-a")]
fn init_hardware() {
    // 变体 A 特定初始化
}

#[cfg(feature = "s31-variant-b")]
fn init_hardware() {
    // 变体 B 特定初始化
}
```

## 固件命名

构建后的固件会根据变体自动命名（如果使用构建脚本）：

    // 变体 B 特定初始化
}
```

## 固件命名

构建后的固件会根据变体自动命名（如果使用构建脚本）：

- `fan-control-variant-a.bin`
- `fan-control-variant-b.bin`

## 注意事项

⚠️ **互斥性**: 同时启用多个变体 feature 会导致编译错误（引脚定义冲突）

⚠️ **默认行为**: 不指定任何变体 feature 时，项目以桌面开发模式编译

✅ **最佳实践**: 使用 `build-variants.sh` 脚本确保配置正确
