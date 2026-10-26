# Photo

应用 ID 为 `photo`，入口为 `scripts/main.lua`，启动器显示 `Photo`。
默认安装位置为 `/system/apps/photo`，照片目录为 `/data/appdata/photo/photos`。
System 应用支持清除数据并保留应用本体。

维护版使用三列网格，在首次显示前构建深色网格，并使用 `assets/icon.jpg` 的相册图标。
浏览、前后切图、删除确认和格式支持沿用上游：JPG、JPEG、LVGL BIN。
上游版本和原始文件哈希见 `SOURCE.json`，Apache-2.0 许可证随应用保留。

`assets/icon.svg` 是可编辑图标原稿。`scripts/assets/photo-icon.cjs` 生成 96×96 JPEG；
`PHOTO_ICON_NODE_MODULES` 可指定包含 Sharp 的 node_modules。

从仓库根目录运行：

```sh
cargo test -p os --test photo --offline
cargo run -p app-pack --offline -- system apps target/system-release --size 3072000
python3 scripts/device/console.py photo-push /absolute/image.jpg --run
```

主机回归使用真实 LVGL、指针事件和图像解码器，覆盖浏览、滚动、单图导航、损坏图、
删除确认与取消、48 次往返和 VM 清理。新 system 镜像包含应用，不包含用户照片。
历史实屏记录位于 `docs/history/records/n5-photo-20261024.md`；当前交付入口见
[应用打包工具](../../../tools/app-pack/README.md)。
