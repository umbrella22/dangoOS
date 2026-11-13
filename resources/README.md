# 工程资源

| 目录 | 内容 | 使用入口 |
| --- | --- | --- |
| [skins](skins) | 四套可分发团子的 BIN、元数据与版本 | `tools/check.sh`、固件发布打包、主页预览 |
| [embedts](embedts) | 外部 EmbedTypescript 导出的应用、自检与集成测试字节码 | TS 固件与宿主集成测试 |
| [screenshots](../docs/media/screenshots/) | README 使用的界面展示图片 | `tools/assets/screenshots.py` 生成 |

`skins` 是设备安装输入。发布打包器将其写入 ZIP 的 `dango-skin/`，安装到 NAND；仓库路径与 ZIP 内目录各自遵守对应工具的契约。

团子抓握与滑落资源使用皮肤格式 v4，需要配套的新固件。新固件仍可读取 v1/v2/v3 皮肤。动作图层、格式与主机预览入口见[团子姿态与滑落](../firmware/os/COMPANION_BALANCE.md)。

当前四套资源版本为 `2026-11-11-image26-crawl`，各含 25 个常规表情和独立手、耳、动作表情图层。原图使用 `gpt-image-2.6-sunburst` 完成修整，经 PhotoCraft 蒙版、轮廓和连接图标精修后，按每角色最多 96 色 RGB565 无抖动量化；压缩与相同数据复用为无损。四个 `fox.bin` 共 2,663,288 字节，各包低于 1 MiB。新增惊慌补丁合计 18,893 字节，其他图像流与调色板逐字节保留。可编辑原稿及生成来源保存在本地 `design/companion-image26/`；重打包入口为 `tools/assets/pack_image26_skins.py`，惊慌补丁入口为 `tools/assets/refine_startled_faces.py`，主机全表情预览入口为 `firmware/os/examples/expression_preview.rs`。

`embedts` 由独立 EmbedTypescript 仓库的 `export-dangoos` 生成，包含所保留产物的 SHA256 清单。OS 不生成或转换字节码。更新时从匹配引擎 checkout 导出应用、自检与宿主夹具及对应清单条目；brain 产物不导入，再运行 `python3 tools/check/compiled_apps.py` 和两种引擎选择的回归。格式与信任边界见 [EmbedTS 接入](../firmware/os/EMBEDTS.md)。

私有原图、字体源文件和生成提示词保存在本地 `design/`。固件内嵌的最小资源位于 `firmware/os/src/assets/`，两者分别管理。
