# Lua 电量计模块

> 上游 esp-claw API 资料的本地中文译文。此处包含上游外接硬件能力；ESP-Mosaico 使用板载快照适配，实际支持范围以 [Lua 硬件接口指南](../../../../../../docs/guides/lua-hardware.md) 为准。上游驱动不参与本项目构建。

上游模块通过已有 `i2c` 模块读取电池电压、电流和电量。支持 **BQ27220**、**MAX17048**；后者只有电压和 SOC，没有电流。

## 调用方式

- `local fuel_gauge = require("lib_fuel_gauge")` 导入模块。
- `fuel_gauge.new({ bus = bus })` 创建电量计，默认 BQ27220；`fuel_gauge.new({ bus = bus, chip = "max17048" })` 指定芯片。
- `gauge:read()` 读取全部值；`gauge:read_voltage_mv()`、`gauge:read_current_ma()`、`gauge:read_soc()` 单独读取。部分芯片不支持电流。
- `gauge:chip()` 查询芯片，`gauge:address()` 查询 I2C 地址，`fuel_gauge.supported_chips()` 列出支持芯片。
- `gauge:close()` 关闭句柄。

## 选项表

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `chip` | string | 芯片名，如 `"bq27220"`、`"max17048"` |
| `borrow` | boolean | 按 `port` 借用已初始化的板级总线，默认 `false` |
| `port` | integer | I2C 端口号 |
| `sda` | integer | SDA GPIO |
| `scl` | integer | SCL GPIO |
| `freq_hz` | integer | I2C 时钟，单位 Hz，默认 `400000` |
| `frequency` | integer | `freq_hz` 的别名 |
| `addr` | integer | 7 位 I2C 地址，默认取决于芯片 |
| `bus` | userdata | 已有 `i2c` 总线句柄，推荐方式 |
| `close_bus` | boolean | `gauge:close()` 时关闭通过 `port`/`sda`/`scl` 创建的总线 |

板级已初始化总线可使用 `fuel_gauge.new({ borrow = true, port = 0, chip = "bq27220" })`；此时 `gauge:close()` 关闭电量计但不停止板级总线。设备地址必须空闲。调用方提供的 `bus` 仍由调用方管理，需等所有使用该总线的设备关闭后再关闭。

使用结束后关闭电量计；垃圾回收和任务退出也会清理未使用句柄。

## 数据格式

- `sample.chip`：芯片名称字符串。
- `sample.voltage_mv`：电压。
- `sample.current_ma`：电流；芯片无电流寄存器时为 nil。
- `sample.soc`：电量百分比。

所选芯片无电流寄存器时，`gauge:read_current_ma()` 报错。

## 上游示例

```lua
local fuel_gauge = require("lib_fuel_gauge")
local i2c = require("i2c")

local bus = i2c.borrow(0)
local gauge = fuel_gauge.new({
    bus = bus,
    chip = "bq27220",   -- 也可使用 "max17048"
})

print("chip:", gauge:chip())
local sample = gauge:read()
print(sample.voltage_mv, sample.current_ma, sample.soc)
gauge:close()
bus:close()
```
