# 工程资源

| 目录 | 内容 | 使用入口 |
| --- | --- | --- |
| [skins](skins/) | 四套可分发团子的 BIN、元数据与版本 | `scripts/check.sh`、固件发布打包、主页预览 |
| [embedts](embedts/) | 外部 EmbedTypescript 导出的 brain、自检与集成测试字节码 | TS 固件与宿主集成测试 |
| [screenshots](screenshots/) | README 使用的界面展示图片 | `scripts/assets/screenshots.py` 生成 |

`skins` 是设备安装输入。发布打包器将其写入 ZIP 的 `dango-skin/`，安装到 NAND；仓库路径与 ZIP 内目录各自遵守对应工具的契约。

`embedts` 由独立 EmbedTypescript 仓库的 `export-dangoos` 生成，包含完整 SHA256 清单。OS 不生成或转换这些文件。更新时从匹配引擎 checkout 整体导出并替换目录，再运行 `python3 scripts/check/compiled_apps.py` 和两种引擎选择的回归。格式与信任边界见 [EmbedTS 接入](../crates/os/EMBEDTS.md)。

私有原图、字体源文件和生成提示词保存在本地 `assets/`。固件内嵌的最小资源位于 `crates/os/src/assets/`，两者分别管理。
