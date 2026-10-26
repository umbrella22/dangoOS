# Factory Album

维护版相册安装在 `/system/apps/album_app`，保留 `album_app` 标识、`Album` 名称及
`/data/appdata/album_app/photos` 数据目录。启动器将其识别为 `Origin::System`；
长按操作仍是清除应用数据并保留出厂应用。

相对 `crates/mosaico-lua/vendor/album_app` 的冻结上游，维护版有三处变化：

- 网格固定三列。
- 首次显示前完成深色网格构建，消除启动白屏。
- 启动器使用 `assets/icon.jpg` 中的相册图标。

浏览、前后切图、删除确认、照片路径、启动参数及运行时长沿用上游逻辑。
`LICENSE` 保留上游授权。

## 图标

`assets/icon.svg` 是可编辑原稿。启动器读取 96×96 RGB JPEG，黑色外角与启动器
背景相接。`scripts/render-album-icon.cjs` 使用 Sharp 生成 JPEG；可选的首个参数
为 PNG 预览输出路径。`ALBUM_ICON_NODE_MODULES` 可指定含 Sharp 的 `node_modules`。

## System 镜像交付

应用文件位于独立的 system 文件系统，编译或更新 main_app 不会更新这些文件。
通过 USB 安装到 `/data/apps/album_app` 会改变来源与长按管理行为，因此正式交付
使用现有 system 镜像的文件覆盖流程：

```sh
cargo run -p mosaico-s31 --example album_system_image --offline -- \
  /absolute/system-before.bin apps/album_app /absolute/new-output-directory
```

工具只操作输入镜像的内存副本，不连接设备。输入必须包含冻结上游的原始相册。
输出前会以只读方式重新挂载 LittleFS，逐文件比较完整目录树，并调用实际启动器
注册流程核对 System 来源及图标解码。只有脚本、manifest、JPEG 图标，以及首次
新增的 assets 目录允许变化。输入无法挂载、相册已被改写、空间不足或核对失败
都会终止；工具不会格式化输入镜像。

输出的 `file-tree-report.json` 记录输入／输出镜像哈希和完整文件树。
设备交付从当前设备备份中提取 system，在确认分区边界后写入候选镜像，再读取
完整 Flash，核对 system 内容及所有其他区域。照片保存在 data 分区。

## 验证入口

```sh
cargo test -p mosaico-s31 --example album_system_image --offline
cargo run -p mosaico-s31 --example n5_album_preview --offline -- \
  apps/album_app/scripts/album_app.lua \
  crates/mosaico-lua/testdata/n5-images /absolute/new-preview-directory
```

预览使用真实 LVGL、指针输入和解码器，覆盖三列浏览、滚动、单图导航、损坏图、
删除确认／取消、48 次往返和退出清理。实屏显示及手势结果单独记录在
`docs/N5_ALBUM_LIVE_20261024.md`。
