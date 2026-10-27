# Lua 按键模块

> 上游 esp-claw API 资料的本地中文译文。此处包含上游外接硬件能力；ESP-Mosaico 使用板载快照适配，实际支持范围以 [Lua 硬件接口指南](../../../../../docs/guides/lua-hardware.md) 为准。上游驱动不参与本项目构建。

`button` 提供按键句柄、事件订阅和事件分发。

## 调用方式

- `local button = require("button")` 导入模块。
- `button.new(gpio_num [, active_level [, long_press_ms [, short_press_ms]]])` 创建句柄。
- `button.on(handle, event, callback)` 订阅事件；支持 `press_down`、`press_up`、`press_repeat`、`press_repeat_done`、`single_click`、`double_click`、`multiple_click`、`long_press_start`、`long_press_hold`、`long_press_up`、`press_end`。
- `button.off(handle [, event])` 移除订阅或清空回调。
- `button.dispatch()` 轮询并分发待处理事件。
- `button.get_key_level(handle)` 读取当前电平。
- `button.close(handle)` 关闭不再使用的句柄。

## 返回值与事件

- `button.new(...)` 返回句柄或 `nil, err`。
- `button.close`、`button.on`、`button.off` 返回 `true` 或 `nil, err`。
- `button.dispatch()` 返回已分发回调事件数量。
- 回调事件表包含 `handle`、`event`、`repeat_count`、`pressed_time_ms`。

## 示例

```lua
local button = require("button")

local handle = button.new(0, 0)
button.on(handle, "single_click", function(evt)
  print(evt.event, evt.repeat_count)
end)

button.dispatch()
button.close(handle)
```
