# Lua IMU 模块

> 上游 esp-claw API 资料的本地中文译文。此处包含上游外接硬件能力；ESP-Mosaico 使用板载快照适配，实际支持范围以 [Lua 硬件接口指南](../../../../../../docs/guides/lua-hardware.md) 为准。上游驱动不参与本项目构建。

`imu` 读取受支持 IMU 的原始加速度、角速度、温度和中断状态。上游可用芯片包括 BMI270、ICM42670、MPU6050，取决于固件配置。

## 板载传感器

```lua
local imu = require("imu")
local sensor = imu.new({ device = "imu_sensor" })
local sample = sensor:read()
print(sample.accel.x, sample.gyro.z)
sensor:close()
```

`device` 选择板级固件支持的 IMU，必要时自动初始化；已有采样设置保留。关闭 Lua 句柄后板载传感器仍可用。同一个板载 IMU 同时只能由一个 Lua 句柄打开。

## 外接传感器

```lua
local i2c = require("i2c")
local imu = require("imu")
local bus = i2c.new(1, 4, 5)
local sensor = imu.new({ chip = "mpu6050", bus = bus, address = 0x68 })
print(sensor:read().accel.z)
sensor:close()
bus:close()
```

控制器和 GPIO 需符合实际开发板。外部传感器也可使用 `i2c.borrow(port)` 总线句柄。

| 字段 | 含义 |
| --- | --- |
| `chip` | 必填，`bmi270`、`icm42670`、`mpu6050` 之一，且固件已启用 |
| `bus` | 必填，Lua I2C 总线句柄 |
| `address` | 可选 7 位地址，默认使用芯片默认地址 |
| `int_gpio` | 可选中断输入 GPIO，默认 `-1` 表示未使用 |
| `sdo_gpio` | 可选地址选择输出 GPIO，默认 `-1` 表示未使用 |

外部传感器使用 400 kHz I2C，地址及可选 GPIO 必须空闲。先关闭传感器再关闭总线；仍有传感器使用时总线不能关闭。

必须传入且只传入一个配置表。板载模式只接受 `device`，不能混用外接字段。未知选项、不可用设备、不支持的芯片和资源冲突会抛出 Lua 错误。

## 方法与清理

- `sensor:read()` 返回 `accel`、`gyro` 表，各含 `x`、`y`、`z`，另有 `sens_time`、`status`。
- `sensor:read_temperature()` 返回芯片原始温度值。
- `sensor:read_int_status()` 返回中断状态位；读取可能清除硬件状态位。
- `sensor:name()` 返回板载设备名，外接设备则返回芯片名。
- `sensor:close()` 释放 Lua 句柄并销毁外接实例；重复成功关闭无副作用。

数值是芯片专属原始读数，不是统一物理单位。BMI270 的 `sens_time` 为传感器计数器，ICM42670 和 MPU6050 使用系统微秒。板载传感器量程遵循板级配置，外接量程取决于芯片。

关闭后或硬件访问失败时，读取方法报错，可用 `pcall` 处理。Lua GC 也会终结句柄，但推荐显式关闭；关闭失败时应先重试再丢弃句柄。
