# Lua 灯带模块

> 上游 esp-claw API 资料的本地中文译文。此处包含上游外接硬件能力；ESP-Mosaico 使用板载快照适配，实际支持范围以 [Lua 硬件接口指南](../../../../../../docs/guides/lua-hardware.md) 为准。上游驱动不参与本项目构建。

`led_strip` 提供 WS2812 等灯带的像素设置和刷新接口。

## 调用方式

- `local led_strip = require("led_strip")` 导入模块。
- `led_strip.new(gpio, max_leds)` 创建灯带句柄。
- `strip:set_pixel(index, r, g, b)` 设置单个像素。
- `strip:set_pixel_hsv(index, h, s, v)` 按 HSV 设置像素。
- `strip:refresh()` 应用修改。
- `strip:clear()` 清空灯带，`strip:close()` 关闭句柄。

像素索引从 0 开始。`set_pixel`、`set_pixel_hsv`、`refresh`、`clear`、`close` 成功时无返回值，失败时抛出 Lua 错误。

`set_pixel_hsv` 的范围为：`h` 取 0–359，`s` 和 `v` 取 0–255。

## 示例

```lua
local led_strip = require("led_strip")

local strip = led_strip.new(8, 1)
strip:set_pixel(0, 255, 0, 0)
strip:set_pixel_hsv(0, 120, 255, 64)
strip:refresh()
strip:close()
```
