# ESP32-S31-Korvo-1 引脚配置待办

## 需要确认的引脚信息

请参考以下资料填写：
- 用户指南: https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp32-s31-korvo-1/user_guide.html
- 原理图: https://dl.espressif.cn/schematics/esp32-s31-korvo-1-schematics.pdf
- LCD 子板文档: https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s3/esp32-s3-lcd-ev-board/user_guide.html

---

## ✅ 已知信息

- **LCD**: 4.3 寸 800×480 RGB 接口 (ESP32-S3-LCD-EV-Board-SUB3)
- **RGB LED**: GPIO37 (WS2812)
- **主控**: ESP32-S31-WROOM-3

---

## ❓ 待确认引脚

### 1. LCD RGB 接口引脚

从原理图中找到 LCD 子板连接器的引脚映射：

```rust
// 需要确认的引脚（当前是占位符）
pub const LCD_PCLK_GPIO: u8 = ??;    // Pixel Clock
pub const LCD_HSYNC_GPIO: u8 = ??;   // H-Sync
pub const LCD_VSYNC_GPIO: u8 = ??;   // V-Sync
pub const LCD_DE_GPIO: u8 = ??;      // Data Enable

// RGB 数据线 (16-bit RGB565 模式)
pub const LCD_R0_GPIO: u8 = ??;      // Red[0]
pub const LCD_R1_GPIO: u8 = ??;      // Red[1]
pub const LCD_R2_GPIO: u8 = ??;      // Red[2]
pub const LCD_R3_GPIO: u8 = ??;      // Red[3]
pub const LCD_R4_GPIO: u8 = ??;      // Red[4]

pub const LCD_G0_GPIO: u8 = ??;      // Green[0]
pub const LCD_G1_GPIO: u8 = ??;      // Green[1]
pub const LCD_G2_GPIO: u8 = ??;      // Green[2]
pub const LCD_G3_GPIO: u8 = ??;      // Green[3]
pub const LCD_G4_GPIO: u8 = ??;      // Green[4]
pub const LCD_G5_GPIO: u8 = ??;      // Green[5]

pub const LCD_B0_GPIO: u8 = ??;      // Blue[0]
pub const LCD_B1_GPIO: u8 = ??;      // Blue[1]
pub const LCD_B2_GPIO: u8 = ??;      // Blue[2]
pub const LCD_B3_GPIO: u8 = ??;      // Blue[3]
pub const LCD_B4_GPIO: u8 = ??;      // Blue[4]
```

**提示**：查看原理图中 "LCD Connector" 部分，或者 ESP32-S3-LCD-EV-Board 的引脚定义。

### 2. 背光控制

```rust
pub const LCD_BL_GPIO: u8 = ??;      // 背光 PWM 控制
```

### 3. 触摸屏接口（如果有）

```rust
pub const TOUCH_SCL_GPIO: u8 = ??;   // I2C 时钟
pub const TOUCH_SDA_GPIO: u8 = ??;   // I2C 数据
pub const TOUCH_INT_GPIO: u8 = ??;   // 中断引脚
pub const TOUCH_RST_GPIO: u8 = ??;   // 复位引脚
```

### 4. 风扇 PWM 引脚

选择一个未被占用的 GPIO 作为风扇 PWM 输出：

```rust
pub const FAN_PWM_GPIO: u8 = ??;     // 扩展 GPIO
```

**候选引脚**（避开已占用的）：
- GPIO 列表：从用户指南的 GPIO 分配表中选择空闲引脚
- 避开：GPIO0-25 (microSD/NAND), GPIO37 (RGB LED)

---

## 📋 填写步骤

### 步骤 1：查看用户指南

访问 https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp32-s31-korvo-1/user_guide.html

找到 "GPIO 分配列表" 章节，记录：
- 哪些 GPIO 已被占用（音频、microSD、按键等）
- 哪些 GPIO 是空闲的（可以用于风扇 PWM）

### 步骤 2：查看 LCD 子板文档

访问 https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s3/esp32-s3-lcd-ev-board/user_guide.html

找到 SUB3 子板的引脚定义：
- RGB 数据线映射
- 同步信号引脚
- 背光控制引脚

### 步骤 3：下载原理图

从 https://dl.espressif.cn/schematics/esp32-s31-korvo-1-schematics.pdf 下载

搜索关键词：
- "LCD Connector" 或 "LCD_CON"
- "RGB" 或 "HSYNC" 或 "VSYNC"
- "Backlight" 或 "BL_PWM"

### 步骤 4：填写到代码

将确认的引脚号填写到：
`src/embedded_hal/board.rs` 的 `variant_b` 模块中

---

## 🔧 更新代码

确认引脚后，运行：

```bash
# 编辑引脚配置
vim src/embedded_hal/board.rs

# 搜索 "variant_b" 并更新引脚号

# 验证编译
./build-variants.sh variant-b
```

---

## 📝 示例参考

如果找不到具体引脚，可以参考类似开发板：
- ESP32-S3-LCD-EV-Board 的引脚映射
- ESP32-S3 官方 BSP 代码中的引脚定义

---

## ❓ 需要帮助？

如果你找到了引脚信息但不确定如何填写，可以：

1. 将引脚列表发给我
2. 我帮你填写到 `board.rs` 中
3. 然后构建固件

**格式示例**：
```
LCD_PCLK = GPIO42
LCD_HSYNC = GPIO39
LCD_VSYNC = GPIO40
...
```
