# 外部编译产物

`embedts/` 由独立 EmbedTypescript 仓库的 `export-dangoos` 生成，包含默认 brain、自检、OS 集成测试夹具及完整 SHA-256 清单。OS 不生成或转换这些文件。

更新时从匹配引擎 checkout 整体导出并替换目录，再运行 `python3 scripts/check/compiled_apps.py` 和两种引擎选择的回归。格式与信任边界见 [EmbedTS 接入](../docs/architecture/embedts-integration.md)。
