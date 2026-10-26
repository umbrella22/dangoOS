# TJpgDec R0.03

TJpgDec R0.03，(C) ChaN，2021，来自 LVGL 9.6 的 `src/libs/tjpgd`（lvgl_binding_rust `cad0311`）。许可条款保留在 `tjpgd.c` 头部。

本地修改：`tjpgd.h` 包含 `<stdint.h>` 并定义 `LV_USE_TJPGD`，替代 LVGL 公开头部，使其能在 LVGL 外独立构建。

LVGL 版本以 B、G、R 字节顺序写入 RGB888 像素，`src/jpeg.rs` 将其交换为 R、G、B。

`build.rs` 将 `jd_prepare`/`jd_decomp` 重命名为 `mosaico_jd_*`。ESP32-S31 ROM 自带的 TJpgDec 使用不同的 `JDEC` 布局；若不重命名，esp-hal ROM 链接脚本可能把调用解析到 ROM 版本。
