# 独立图像测试样本

这些样本在 N5 解码器实现之前，由独立验收流程使用 Pillow 生成。`receipt.json`
记录文件哈希和准确的像素输入。图像包含合成的方向、颜色、灰度和透明度图案，以及
刻意截断或损坏的变体，不含外部摄影素材。

测试逐字节比较 PNG 解码结果；JPEG 使用历史验收产物中记录的独立 Pillow 对照。
PNG 透明像素与黑色背景合成，通道计算公式为 `(channel * alpha + 127) / 255`。

`orientation-rgb565.bin` 是独立的 LVGL 9 RGB565 文件，包含 12 字节头部和小端像素；
对应记录描述了二进制字段和哈希。Photo 集成测试使用该文件，与 Lua 图像解码器测试
分别执行。原先的一次性探针及配套脚本已退出维护，当前回归入口为：

```sh
cargo test -p lua-engine --offline
cargo test -p os --test photo --offline
```

`red_blue_16x8.jpg` 是使用 Pillow 生成的 16×8 红蓝分区 JPEG，由 `src/jpeg.rs` 的单元
测试验证颜色方向和截断输入处理。其余样本的来源记录覆盖范围以对应 JSON 的文件清单为准。
