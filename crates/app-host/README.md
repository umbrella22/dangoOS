# app-host

dangoOS 应用共享的数据契约、栅格绘制、字体与图像编解码。该 crate 使用 `no_std + alloc`，
不依赖 Lua、QuickJS、LVGL 或设备 HAL。

`RawAllocator`、硬件/服务快照、堆指标和 UI 请求/回执供 OS 及所选引擎共同使用。
JPEG 的 TJpgDec 与 stb 实现在这里单独构建；Lua 的图像模块通过自有适配器继续
将解码工作缓冲区计入其 VM 账本。Lua 原有公开类型和 `jpeg` 模块保持重导出。

```sh
cargo test -p app-host --features graphics
```

`graphics` feature 构建与 Lua 相同的 raster、bitmap、内置字体、DFN1 与 PNG 内核，
导出符号加前缀以供两种引擎的主机差分测试同时链接。TS 原生图像配额和编解码
暂存预算由 OS 的 `ts_graphics` 适配器检查，不属于 QuickJS 分配账本。
