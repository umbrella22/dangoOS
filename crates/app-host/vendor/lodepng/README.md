# LodePNG

来源：<https://github.com/lvandeve/lodepng>，提交 `e584a8490db1b76b81bf298c11c09d3b55d9c270`。

`lodepng.c`、`lodepng.h` 保留上游 zlib 许可证和源码声明。构建时为导出符号增加私有前缀，使其与 LVGL 自带 LodePNG 共存，并将分配计入所属 Lua VM 的受限图像内存账本。PNG 解码使用有界输出分配；RGBA 通过带舍入的整数运算合成到黑色背景后，作为 RGB888 帧提供。
