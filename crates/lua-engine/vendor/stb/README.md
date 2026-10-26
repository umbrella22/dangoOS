# stb_image_write

`stb_image_write.h` 原样复制自 <https://github.com/nothings/stb>，提交 `a7d4c2175630e1ea5b6f8dd00ab96fed0b97b96d`。

头文件保留 MIT/公有领域双许可证。项目只使用内存 JPEG 编码器，全部导出为私有静态符号，禁用 stdio，通过分配宏将临时编码缓冲计入所属 VM 的内存账本。JPEG 转换使用质量 90 的基线编码，输出上限为 4 MiB。
