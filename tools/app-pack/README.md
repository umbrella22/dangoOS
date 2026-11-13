# app-pack

从仓库应用源码创建新的 LittleFS system 镜像。工具在主机内存中格式化新文件系统，
写入指定应用根目录下的全部应用；输出目录必须尚不存在。默认应用集由 `apps/`
决定，当前为 snake、2048、dino、flappy、photo，入口位于 `apps/<分类>/<id>/scripts/main.lua`。
TS 应用由外部 EmbedTypescript 构建为 `.etb` 应用树。

在仓库根目录执行：

```sh
cargo run -p app-pack --offline -- check apps/builtin
cargo run -p app-pack --offline -- skins resources/skins
# size 是设备分区表中的 system 分区字节数；当前布局为 496 个 4 KiB 块
cargo run -p app-pack --offline -- system apps/builtin target/system-release --size 2031616
cargo run -p app-pack --offline -- extract /absolute/system.bin /absolute/new-extraction
# 可独立传输的单个应用包、完整应用集和字体资源
cargo run -p app-pack --offline -- bundle apps/builtin/games/snake target/snake-bundle
cargo run -p app-pack --offline -- verify-bundle target/snake-bundle
cargo run -p app-pack --offline -- bundles apps target/application-bundles
cargo run -p app-pack --offline -- fonts target/ui-fonts
cargo test -p app-pack --offline
# TS 应用先生成字节码，输出目录必须尚不存在
(cd ../EmbedTypescript && python3 scripts/build_examples.py target/apps)
cargo run -p app-pack --offline -- bundles ../EmbedTypescript/target/apps target/application-bundles-ts
cargo run -p app-pack --no-default-features --features ts-engine --offline -- system ../EmbedTypescript/target/apps target/system-ts --size 2031616
```

`system` 输出 `system.bin`、`file-tree-report.json` 和 `application-docs/<id>/`。
应用根目录的 README、LICENSE、SOURCE.json 作为主机交付资料保存，其余非隐藏
文件写入 `/apps/<id>/`。角色皮肤全部独立安装到 NAND，system 不包含 `/skins`。
固件内置一张狐狸团子虚影与问号，在缺少角色资源时显示。完整 UI 字体写入 `/ui/fonts-<SHA256>.bin`，
字体数据和 recovery 子集由同一份 MFNT 输入生成，固件只接受构建时固定的哈希。照片、存档、开发示例和设备探针不进入 system 分区。
源码中新增一个分类下的有效应用后，下一次打包会自动将其加入应用集。

工具重新只读挂载输出，逐项比较文件树，校验全部应用的注册、System 来源、模块依赖
和图标解码。报告记录完整文件清单、大小、SHA-256、空闲块数和应用资料哈希。
相同应用输入及分区大小产生逐字节相同的镜像。分区容量不足时明确报出失败路径。

`extract` 读取既有 LittleFS 镜像，写入新的主机目录并核对文件字节；输入不被修改。
此命令用于检查镜像，不参与新 system 镜像的构建。

`skins` 校验公开的 NAND 团子资源目录：每套恰好包含 `fox.bin` 和 `skin.json`，
角色 ID、版本、画布、大小和 CRC 符合设备契约，全部图像可被设备解码器解析。
输出 JSON 清单包含每套资源的名称、版本、文件大小与 SHA-256，供发布打包器使用。
四套资源（含 `default` 心月狐）由发布包中的 USB 安装器写入 NAND，不加入 NOR 的 system 镜像。
`placeholder` 是内置虚影的保留 ID，不能用资源包覆盖。

应用正常运行的存储根为 `/data/appdata/<id>`；USB 下载更新写入 `/data/apps/<id>`，
同 ID 下载版优先于 system 版本。删除下载版后显示 system 版本，清数据保留应用本体。
固件只按当前 ID 扫描文件系统，不执行旧应用 ID 或数据目录迁移。

镜像构建与设备写入分开执行。分区大小必须与目标设备的实际分区表一致；主机验证结果
不代表已完成设备上的升级、输入或显示验收。

`bundle` 输出 `payload/`、可选 `documentation/` 和 `bundle.json`。运行文件清单、字节数与
SHA-256 覆盖整个 payload；`verify-bundle` 重新验证真实 manifest、入口、图标和文件内容。
`bundles` 对正式应用集逐个执行同一导出流程。`fonts` 导出与当前固件匹配的完整 UI 字体包。
输出都是独立目录，可以直接压缩或随发布 ZIP 交付，不包含编译器或旧设备镜像。

```sh
python3 tools/device/console.py bundle-push target/snake-bundle --port RUNTIME_PORT
python3 tools/device/console.py fonts-push target/ui-fonts --port RUNTIME_PORT
```

应用上传使用现有 Data 应用暂存/提交协议。字体上传在设备端核对完整 SHA-256 和四份
MFNT 的表与像素边界，再按内容地址原子提交，重启时加载；不删除旧哈希资源。
原生 Rust 代码由 Cargo 静态链接，应用包不承载可动态执行的原生二进制。

`system` 会在所选引擎的注册表中验证镜像应用可运行。打包器默认选择 Lua；TS system 镜像使用 `--no-default-features --features ts-engine`。`check` 与独立 bundle 校验两种入口格式，不调用编译器。
