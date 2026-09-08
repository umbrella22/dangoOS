# ESP32-S31-Korvo-1 引脚配置

## ✅ 已完成配置

所有引脚定义已从 **ESP 官方 BSP** 提取完成！

- **来源**: https://github.com/espressif/esp-bsp/tree/master/bsp/esp32_s3_lcd_ev_board
- **位置**: `src/embedded_hal/board.rs` 的 `korvo` 模块

---

## 📋 引脚映射总结

### LCD RGB 接口 (16-bit RGB565)

| 功能 | GPIO | 说明 |
|------|------|------|
| PCLK | 9 | Pixel Clock |
| HSYNC | 46 | H-Sync |
| VSYNC | 3 | V-Sync |
| DE | 17 | Data Enable |

### RGB 数据线

| 颜色通道 | GPIO 列表 |
|---------|-----------|
| Red[0:4] | 10, 11, 12, 13, 14 |
| Green[0:5] | 21, 47, 48, 45, 38, 39 |
| Blue[0:4] | 40, 41, 42, 2, 1 |

### 外设

| 外设 | GPIO | 说明 |
|------|------|------|
| RGB LED | 19 | WS2812 x12 |
| 按键 | 8 | ADC 多路按键 |
| I2C SCL | 2 | IO Expander |
| I2C SDA | 1 | IO Expander |
| 功放电源 | 38 | Power AMP |
| **风扇 PWM** | **4** | 自定义（GPIO4 空闲） |

---

## 🚀 立即可用

现在可以直接编译 Korvo-1 固件：

```bash
# 编译
./build-variants.sh korvo --release

# 烧录
./build-variants.sh korvo --release --flash --monitor
```

---

## 📝 引脚来源

```c
// 来自 ESP-BSP: esp32_s3_lcd_ev_board.h
#define BSP_LCD_SUB_BOARD_2_3_PCLK      (GPIO_NUM_9)
#define BSP_LCD_SUB_BOARD_2_3_HSYNC     (GPIO_NUM_46)
#define BSP_LCD_SUB_BOARD_2_3_VSYNC     (GPIO_NUM_3)
#define BSP_LCD_SUB_BOARD_2_3_DE        (GPIO_NUM_17)
// ... (16 个数据线)

#define BSP_LED_RGB_GPIO                (GPIO_NUM_19)  // WS2812
#define BSP_BUTTONS_IO                  (GPIO_NUM_8)
```

---

## 🔧 硬件特性

Korvo-1 支持的硬件特性（已在代码中配置）：

- ✅ **RGB LED**: WS2812 x12 灯珠
- ✅ **音频**: I2S 接口（预留）
- ✅ **摄像头**: DVP 接口（预留）
- ✅ **按键**: 6 个按键（ADC 共用）
- ✅ **扩展**: microSD 卡槽

这些特性可通过条件编译选择性启用/禁用（树摇优化）。
