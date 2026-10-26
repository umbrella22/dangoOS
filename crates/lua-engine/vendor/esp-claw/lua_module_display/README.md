# `display` Lua 绘图 API

> 上游 esp-claw API 的本地中文译文。本项目对帧呈现等待和内置字体选择有适配，见[来源与修改说明](../../NOTICE.md)。ESP-Mosaico 的应用入口和运行限制见 [Lua 应用指南](../../../../../docs/guides/lua-apps.md)。

`display` 是内置屏幕的即时模式二维绘图 API，适用于游戏、仪表盘、相机叠加层和自定义界面，不提供控件或外接屏幕 API。同一时刻只有一个 Lua 任务使用屏幕，返回的屏幕对象只能由打开它的任务使用。下方签名用 `->` 表示返回值，未标注的函数无返回值。

## 快速开始

```lua
local display = require("display")
local screen <close> = display.open()
local info = screen:info()
local panel_color = display.color(20, 90, 130)

screen:begin({ clear = "#101820" })
screen:fill_round_rect(12, 12, info.width - 24, 60, 8, panel_color)
screen:text(24, 30, "hello", { font_size = 24, color = 0xFFFFFFFF })
screen:present()
```

绘制顺序固定为 `begin()` → 绘制 → `present()`。`screen:close()`、Lua `<close>` 作用域或任务退出会释放屏幕；任务运行中仅丢弃 Lua 变量不会释放。场景需要继续可见时，屏幕应保持打开。

## 打开屏幕与查询信息

`display.open([options]) -> screen` 打开内置屏幕，当前屏幕关闭前再次调用会报 `display already open`。

| 选项 | 默认值 | 含义 |
| --- | --- | --- |
| `framebuffer_count` | `1` | 取 `1` 或 `2`；场景确有需要且可承担额外内存时才使用 `2` |

绘制格式随内置屏幕色深决定，无需配置面板字节序。`screen:info()` 返回：

| 字段 | 含义 |
| --- | --- |
| `width`, `height` | 屏幕像素尺寸 |
| `pixel_format` | 绘制格式 `"rgb565"` 或 `"rgb888"` |
| `bytes_per_pixel` | RGB565 为 `2`，RGB888 为 `3` |
| `framebuffer_count` | 当前帧缓冲数量 |
| `framebuffer_bytes` | 全部帧缓冲预留的总字节数 |
| `touch_available` | 内置屏幕是否有触摸输入 |

逐帧需要这些信息时可缓存该表。`screen:stats()` 返回最近一帧统计：

| 字段 | 含义 |
| --- | --- |
| `draw_us` | 从 `begin()` 开始到 `present()` 开始的时间，含帧缓冲同步、清屏、Lua 场景逻辑和绘制 |
| `present_us` | 提交像素并等待呈现的耗时 |
| `sync_us` | `draw_us` 中用于同步双帧缓冲的部分 |
| `dirty_pixels` | 提交的像素数量 |
| `dirty_rects` | 提交的矩形数量 |
| `submitted_bytes` | 面板字节序转换前提交的像素字节数 |
| `framebuffer_bytes` | 全部帧缓冲预留的总内存 |

无更新帧的呈现耗时、矩形数、像素数和提交字节数均为零。`screen:close()` 可重复调用；关闭后调用其他屏幕方法会报错。

## 帧与绘制状态

```lua
screen:begin({ clear = 0xFF0C1520 })
screen:save()
screen:translate(20, 40)
screen:clip(0, 0, 120, 80)
screen:fill_rect(0, 0, 120, 80, 0xCC183047)
screen:restore()
local updated = screen:present()
```

- `screen:begin([{ clear = color }])` 开始一帧，省略 `clear` 则保留前图，首次绘制从黑色开始。当前帧未呈现就开始新帧会报错。
- `screen:present([{ full = false }]) -> boolean` 完成一帧，提交了像素时返回 `true`。无变化通常返回 `false`；`full = true` 请求全屏刷新。呈现失败后帧仍活动，可重试或关闭屏幕。
- `screen:save()` / `screen:restore()` 保存/恢复平移和裁剪，最多 8 层；无匹配保存的恢复会报错。
- `screen:translate(dx, dy)` 为后续坐标添加整数偏移。`screen:clip(x, y, w, h)` 将当前裁剪与平移坐标中的矩形相交；负宽高报错，零宽高形成空裁剪。每次 `begin()` 重置绘制状态。

绘制、平移、裁剪、保存和恢复需要活动帧；`info()`、`stats()`、`measure_text()`、`touch()` 不需要。

## 颜色与形状

形状方法的 `color` 接受：

- 打包的 `0xAARRGGBB` 整数。在 32 位 Lua 整数设备上，最高位置位时可能显示为负数，但颜色位不变。
- `"#rrggbb"` 或 `"#rrggbbaa"` 字符串。
- `{ r = 255, g = 80, b = 40, a = 192 }` 或 `{ 255, 80, 40, 192 }`；省略 alpha 时默认为 255。

`display.color(r, g, b[, a]) -> integer` 将 0..255 通道打包为可复用颜色，`a` 默认 255。alpha 为 0 时不绘制，255 时不透明，中间值与当前画面混合。整数字面量必须包含 alpha：不透明红色用 `0xFFFF0000`，`0xFF0000` 缺少 alpha。六位字符串 `"#ff0000"` 为不透明色。

位置、尺寸和半径使用整数。原点在左上，x 向右增加、y 向下增加；绘制受屏幕边界和当前裁剪限制。

```lua
screen:fill_rect(x, y, w, h, color)
screen:stroke_rect(x, y, w, h, color)
screen:line(x0, y0, x1, y1, color)
screen:fill_circle(cx, cy, radius, color)
screen:stroke_circle(cx, cy, radius, color)
screen:arc(cx, cy, radius, start_deg, end_deg, color)
screen:fill_round_rect(x, y, w, h, radius, color)
screen:stroke_round_rect(x, y, w, h, radius, color)
screen:fill_triangle(x1, y1, x2, y2, x3, y3, color)
```

圆弧角度必须有限，在屏幕坐标中顺时针增加。描边宽度为 1 像素。矩形尺寸非正或半径为负时不绘制。圆弧跨度至少 360 度时绘制整圆，起止相同时不绘制，负差值按顺时针绕过 360 度。

## 文本与字体

```lua
local w, h = screen:measure_text("status", { font_size = 24 })
screen:text(8, 8, "status", { font_size = 24, color = "#ffffff" })

local font <close> = display.load_font(font_path)
screen:text(8, 40, "Temperature", { font = font, color = "#ffffff" })
```

`screen:text(x, y, text[, options])` 在活动帧内绘制有效 UTF-8。`screen:measure_text(text[, options]) -> width, height` 使用同样尺寸规则，可在帧外调用。两者接受 `color`（默认白）、`font_size`（默认 24）和 `font`。

未指定 `font` 时，内置字体支持可打印 ASCII，整数尺寸为 8–64。文本不自动换行或对齐：`\n` 开新行，`\r` 返回当前行开头，`\t` 前进 4 个字形宽。空字符串尺寸为 `0, 0`。

其他 Unicode 字符可通过 `display.load_font(path) -> font` 加载可读 DFN1 位图字体。DFN1 保留每字形的前进量、边界框、基线偏移和 Unicode 编码。自定义字体按存储尺寸渲染，指定 `font` 后忽略 `font_size`。缺字时优先使用字体配置的回退字形，否则绘制和度量报错；无效 UTF-8 也报错。`font:close()` 幂等，关闭字体不可再用；最后一次同步 `text()`/`measure_text()` 调用结束后才能关闭。

上游 `bdf_font_converter` 技能可在设备上把可写存储中的横排 Unicode BDF 2.1/2.2 转为 DFN1，该工具未作为本仓库内置应用提供。格式限制行高、字形宽高和水平前进量不超过 64 像素，最多 4096 个字形，文件上限 1 MiB。

## 图像与原始像素

```lua
local image = require("image")
local frame <close> = image.load_file(image_path)
local red_tile = string.rep("\0\248", 64 * 64)
screen:begin()
screen:image(0, 0, frame, { mode = "contain", width = 120, height = 90, opacity = 220 })
screen:blit(130, 0, red_tile, { width = 64, height = 64, format = "rgb565" })
screen:present()
```

`screen:image(x, y, image_frame[, options])` 接收 `image` 模块的 `image.frame`，调用期间帧必须有效。绘制同步完成，`display` 不保留图像帧。

| `mode` | 效果 |
| --- | --- |
| `"raw"`（默认） | 按源区域原尺寸绘制，忽略目标 `width`/`height` |
| `"contain"` | 不变形地完整放入目标框并居中 |
| `"cover"` | 不变形地填满目标框，从中心裁剪 |
| `"stretch"` | 填满目标框，允许变形 |
| `"crop"` | 从源区域左上角绘制，不缩放，限制在目标尺寸内 |

选项还包括 `source = { x = 0, y = 0, width = 32, height = 24 }` 和 0..255 的 `opacity`（默认 255）。源坐标必须在图像内，区域必须能完整容纳；省略源尺寸时延伸到源图边缘。非 `raw` 模式省略目标尺寸或设为零时，默认使用所选源尺寸。透明度为零不绘制。

目标框不会自动清空，例如 `contain` 未覆盖区域保留原画面。

`screen:blit(x, y, bytes, options)` 绘制 Lua 字符串中的原始像素。必须提供正整数 `width`、`height` 和 `format`：`"rgb565"` 为小端像素，`"rgb888"` 为 RGB 字节顺序，`"bgr888"` 为 BGR。字符串长度必须精确等于 `width * height * bytes_per_pixel`。`blit()` 不缩放、不应用透明度，此类操作使用 `image()`；不接受原始指针。

`image()`、`blit()` 均遵守平移和裁剪，坐标指定目标左上角。

## 触摸与错误

`screen:touch() -> { points = { { id = number, x = number, y = number }, ... } }` 返回屏幕坐标中的最新触摸快照。`points` 按提供方顺序包含全部活动触点，无接触时为空。使用 `id` 跟踪手势，数组位置不保证稳定。无内置触摸时调用报不支持，可先检查 `screen:info().touch_available`。

非法参数、无效 UTF-8、错误帧状态、已关闭屏幕/字体和硬件不可用都会抛出 Lua 错误；需要恢复预期失败时使用 `pcall`。动画宜一次加载图像和字体、复用颜色与像素字符串，每帧只用一组 `begin()` / `present()`。
