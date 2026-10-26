# app-pack

从仓库应用源码创建新的 LittleFS system 镜像。工具在主机内存中格式化新文件系统，
写入指定应用根目录下的全部应用；输出目录必须尚不存在。默认应用集由 `apps/`
决定，当前为 snake、2048、dino、flappy、photo。

在仓库根目录执行：

```sh
cargo run -p app-pack --offline -- check apps
# size 是设备分区表中的 system 分区字节数；当前板卡为 750 个 4 KiB 块
cargo run -p app-pack --offline -- system apps target/system-release --size 3072000
cargo run -p app-pack --offline -- extract /absolute/system.bin /absolute/new-extraction
cargo test -p app-pack --offline
```

`system` 输出 `system.bin`、`file-tree-report.json` 和 `application-docs/<id>/`。
应用根目录的 README、LICENSE、SOURCE.json 作为主机交付资料保存，其余非隐藏
文件写入 `/apps/<id>/`。镜像只包含应用本体；照片、存档、开发示例和设备探针不进入
system 分区。源码中新增一个分类下的有效应用后，下一次打包会自动将其加入应用集。

工具重新只读挂载输出，逐项比较文件树，校验全部应用的注册、System 来源、模块依赖
和图标解码。报告记录完整文件清单、大小、SHA-256、空闲块数和应用资料哈希。
相同应用输入及分区大小产生逐字节相同的镜像。分区容量不足时明确报出失败路径。

`extract` 读取既有 LittleFS 镜像，写入新的主机目录并核对文件字节；输入不被修改。
此命令用于检查镜像，不参与新 system 镜像的构建。

应用正常运行的存储根为 `/data/appdata/<id>`；USB 下载更新写入 `/data/apps/<id>`，
同 ID 下载版优先于 system 版本。删除下载版后显示 system 版本，清数据保留应用本体。
固件只按当前 ID 扫描文件系统，不执行旧应用 ID 或数据目录迁移。

镜像构建与设备写入分开执行。分区大小必须与目标设备的实际分区表一致；主机验证结果
不代表已完成设备上的升级、输入或显示验收。
