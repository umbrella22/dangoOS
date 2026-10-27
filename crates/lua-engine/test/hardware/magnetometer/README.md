# Lua 磁力计模块

> 上游 esp-claw API 资料的本地中文译文。此处包含上游外接硬件能力；ESP-Mosaico 使用板载快照适配，实际支持范围以 [Lua 硬件接口指南](../../../../../docs/guides/lua-hardware.md) 为准。上游驱动不参与本项目构建。

`magnetometer` 读取 BMM150、BMM350、QMC6309 的磁场采样，对应芯片必须在固件中启用。

## 板载传感器

```lua
local magnetometer = require("magnetometer")
local sensor = magnetometer.new({ device = "magnetometer_sensor" })
local sample = sensor:read()
print(sample.magnetic.x, sample.magnetic.y, sample.magnetic.z)
sensor:close()
```

指定设备必须由板级固件支持，必要时自动初始化；打开已初始化传感器时保留采样设置。关闭 Lua 句柄后板载传感器继续活动。同一板载传感器同时只能由一个 Lua 句柄打开，Lua 使用期间传感器及总线必须持续可用。

## 外接传感器

```lua
local i2c = require("i2c")
local magnetometer = require("magnetometer")
local bus = i2c.new(1, 4, 5) -- 选择本开发板可用的控制器和引脚。
local sensor = magnetometer.new({ chip = "bmm150", bus = bus, address = 0x10 })
print(sensor:read().magnetic.x)
sensor:close()
bus:close()
```

外接传感器也可使用 `i2c.borrow(port)` 总线。

| 字段 | 含义 |
| --- | --- |
| `chip` | 必填，`bmm150`、`bmm350` 或 `qmc6309` |
| `bus` | 必填，Lua I2C 总线句柄 |
| `address` | 可选 7 位地址：BMM150 为 `0x10`–`0x13`，默认 `0x10`；BMM350 为 `0x14`–`0x15`，默认 `0x14`；QMC6309 为 `0x7c` |
| `int_gpio` | 可选中断输入 GPIO，默认 `-1` 未使用，不安装回调 |
| `sdo_gpio` | BMM150/BMM350 可选地址选择输出 GPIO，默认 `-1` 未使用 |

外接传感器使用 400 kHz 传输，地址和请求 GPIO 必须空闲，不探测其他地址。BMM150 另一个地址选择引脚必须已经匹配请求地址，QMC6309 不接受 `sdo_gpio`。

必须传入且只传入一个配置表。板载模式只接受 `device`，不能混用外接字段。非法选项、不可用设备、未启用芯片、资源冲突和硬件失败均抛出 Lua 错误。

## 读取与关闭

- `sensor:read()` 返回 `magnetic = {x, y, z}`、`raw_magnetic = {x, y, z}`、`status`、`calibrated`；BMM350 还返回摄氏度 `temperature`，其他芯片省略。
- `sensor:read_temperature()` 返回 BMM350 摄氏温度，其他芯片报错。
- `sensor:read_int_status()` 返回芯片专属状态位，读取可能清除硬件状态。
- `sensor:chip_id()` 返回芯片标识。
- `sensor:name()` 返回板载设备名或外接芯片名。
- `sensor:close()` 释放 Lua 句柄并删除外接传感器；重复成功关闭无副作用。

BMM150/BMM350 磁场单位为微特斯拉，QMC6309 为有符号原始计数。`raw_magnetic` 表示应用校准之前的值，Bosch 传感器已应用出厂补偿；校准后的 `magnetic` 单位相同。不计算航向或倾斜补偿。

读取同步执行。QMC6309 以 100 ms 轮询截止时间等待数据就绪，进行中的 I2C 传输和调度可能延长实际耗时。BMM150/BMM350 读取当前采样，调用方需留出启动时间并选择合适轮询间隔。不能通过其他驱动并发访问同一传感器。

先关闭外接传感器再关闭总线，仍有活动传感器时总线不能关闭。GC 也释放句柄，但推荐显式关闭；关闭失败先重试再丢弃。关闭后读取和校准方法失败。

## 校准

校准只影响当前 Lua 对象，不自动保存。应用可保存 `calibration_get()` 结果，再用 `calibration_set()` 恢复。

- `calibration_reset()` 清空校准并开始新采集。
- `calibration_add_sample([sample])` 接收应用校准前的 `{x, y, z}` 数组，省略时读取采样，返回累计采样数。
- `calibration_finish()` 计算偏移和各轴缩放并返回校准表，要求至少 16 个采样且每轴跨度非零；采集时需将传感器旋转到不同方向。
- `calibration_get()` 返回 `hard_iron`（3 数数组）、`soft_iron`（3×3 数组）、`calibrated`、`collecting`、`sample_count`。
- `calibration_set({hard_iron = {...}, soft_iron = {{...}, {...}, {...}}})` 应用有限数值系数并返回校准表。
- `calibration_clear()` 清空校准和采集状态并返回校准表。

修正公式为 `soft_iron * (raw_magnetic - hard_iron)`。无校准时 `magnetic` 等于 `raw_magnetic`，保存的系数应对应具体传感器及安装方式。
