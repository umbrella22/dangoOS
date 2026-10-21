# 素材需求清单

狐狸团子已经覆盖全部 16 个表情（外加 `annoyed`、`charging` 两张额外姿势），
早期 grok-ball 小球时代用别的表情顶替的条目都已解决，原清单见
[legacy/ASSET_REQUESTS_grok_ball.md](legacy/ASSET_REQUESTS_grok_ball.md)。

## 新增或修改狐狸素材的规格

素材流程见 [assets/fox-dango/README.md](../assets/fox-dango/README.md)，管线讲解见
[FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)。要点：

| 项 | 要求 |
| --- | --- |
| 原稿 | 1254x1254 PNG，纯黑背景，放在 `assets/fox-dango/source/<表情>.png` |
| 一致性 | 所有表情同一只狐狸、同一大小和位置（统一几何按 18 张的并集算，单张偏移会让切换时身体跳动） |
| 局部变体 | 只改一处的对齐编辑，命名 `source/variants/<表情>__<部位>.png`（如 `idle__blink`），其余像素必须与底图一致 |
| 符号 | Zzz、汗滴、问号等画在底图上即可，`fox_parts.py` 会与“无符号版”做差抠成精灵 |
| 颜色 | 不必自己压色；全部素材共用一套 128 色调色板，大面积平滑渐变会被量化成色带 |

## P1：官方点阵字体（启动页 `ikaros` 用，当前未启用）

现在的 `ikaros` 是手写的 5x7 点阵字体
（`crates/mosaico-s31/src/ui/splash/font.rs`），刻意贴近官方的点阵观感，
但**不是官方的同一套字形**。官方 splash 的字体在 bootloader 的
`splash` blob 里是**位图**，不是字体文件。

| 素材 | 说明 | 规格 |
| --- | --- | --- |
| `mosaico-splash-font.png` | 官方 splash 的原始位图（从 `mosaico_boot_splash.bin` 提取，或对屏幕拍照后二值化） | PNG，1:1 像素，黑色背景白色字 |
| 可选：`ikaros-atlas.png` | 如果希望按官方字形重画 `ikaros` | 同上，6 个字母横向排列 |

给了之后只需要替换 `ui/splash/font.rs` 里的 `glyph()` 表，动画逻辑不用动。
注意启动页目前关闭（`ui::SHOW_SPLASH = false`），重新启用还需要让 plane 路径能显示它。
