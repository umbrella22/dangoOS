# 固件发布

`Firmware` 工作流在 main、codex 分支和 PR 上执行统一检查、固件构建及发布包校验。
推送 `v<workspace.package.version>` tag 后，同一工作流上传草稿 Release，下载全部附件核对，
成功后公开发布。只有发布 job 获得 `contents: write` 权限。
CI 使用源码提交时间作为固件的构建时间，避免同一提交仅因重跑而改变应用 descriptor。

| 文件 | 职责 |
| --- | --- |
| `inputs.json` | 官方底包、BL/工厂资源与 CI 工具链的固定版本、地址和 SHA-256 |
| `partitions.csv` | ESP-Mosaico 16 MiB A/B 布局，与设备迁移契约逐字段核对 |
| `bootstrap.py` | 安装 Linux CI 的固定 GCC 和 espflash，并覆盖本机工具链路径 |
| `package.py` | 生成和校验 OTA BIN、全量 BIN、两个 ZIP、manifest、安装说明与校验和 |
| `install.py` | 校验解压后的发布包，可先刷写全量 NOR，再通过运行时 USB 安装团子、UI 字体和可选应用更新 |
| `publish.py` | 验证远程 tag 与源码提交，上传并回读附件，再公开 Release |
| `notes/` | 各 tag 的发布说明 |

## 镜像边界

OTA BIN 是应用镜像，通过运行时 USB 控制台或 Wi-Fi HTTPS 下载写入非活动槽。无线更新从 GitHub Releases 读取 manifest，支持无认证 HTTP CONNECT 代理，设备只向用户提供更高且兼容的稳定版本。它不更新 BL、分区表或 system。
全量包从固定哈希的公开官方底包提取 BL 和 Vibe Mode，使用仓库源码构建 system，
填充双 6 MiB A/B 分区与 slot 0 应用。原工厂 UI 区域用于扩大应用槽，不进入新镜像。
初始 OTA 记录为 `sequence=1, state=Valid`。其余空间以 `0xff` 初始化，包含空白
slot 1、NVS 和崩溃记录。可写 data 位于独立 SPI NAND，不占 NOR 分区。

打包过程校验芯片、应用版本、A/B 标记、应用校验和、分区表 MD5、各组件偏移与边界、
system 文件树、全部独立角色包和附件 SHA-256。全量镜像固定 16 MiB，从地址 0x0
写入并替换 NOR 内容，不改写 NAND。新 NAND 在固件运行后使用 `nand-format` 显式初始化。
实际设备上的首装、BL 和恢复路径验收单独记录；构建通过不替代硬件验收。

## NAND 团子分发

公开构建输入为根目录 [`dango-skin/`](../../dango-skin/)，每个角色目录只包含生成后的
`fox.bin` 和 `skin.json`。鸣潮 · 心月狐团子（`default`）、玄灵 · 月蚀、
玄灵 · 常态、智谱团子全部安装到 NAND 的 `/data/skins/<id>/`，不占用 A/B 固件槽或 system 分区。
固件内置一张狐狸团子虚影与问号；未安装或无法读取所选角色时显示该图。
构建和发布只读取这些生成产物，不依赖原图、提示词或私有制作工具。

`app-pack skins dango-skin` 使用设备同一套清单解析器和图像解码器校验每个资源包。
打包器将全部 BIN/JSON 放入 OTA 和全量两个 ZIP，并在 `manifest.json.nand_skins` 中
记录版本、大小和 SHA-256。独立验证器重新检查归档内资源、清单 CRC 和哈希。

发布 ZIP 解压后执行：

```sh
python3 -m pip install -r requirements.txt
# 安装团子与匹配的 UI 字体
python3 install.py --port RUNTIME_PORT
# 全量安装：设备先进入 ROM 下载模式
python3 install.py --flash-port ROM_PORT --port RUNTIME_PORT
```

NOR 和 NAND 是两个独立介质。安装器完成 NOR 刷写并等待运行时 USB 就绪后，通过既有
`skin-begin → file → skin-commit` 协议逐套写入 NAND。每套提交前由设备校验，
保留其他用户文件及 `/data/skins/.selected`，不会自动更换当前团子。
设备启动时的 NAND 挂载探测结束后才允许安装；探测期间安装器只轮询只读状态。
NAND 未挂载时默认退出，只有显式增加 `--format-nand` 才会格式化未挂载的卷。
该操作会擦除 NAND 数据，已挂载的卷不受此参数影响。

无线 OTA 只取独立应用 BIN，保留已有 NAND 资源；需要更新团子资源时使用 ZIP 内的
USB 安装器。心月狐也通过独立包交付；Lua 应用可通过独立 bundle 写入 Data，作为 system 同 ID 应用的更新。
首次从 system 心月狐迁移时，缺少 NAND `default` 包会显示虚影；补装后重新加载或重启恢复。
已有选择记录不改写，旧 system 中的完整皮肤不再读取；内置 `placeholder` 用于资源缺失时兜底，
不出现在设置的团子候选列表中。

两个 ZIP 都包含 `applications/<id>/`、`ui-fonts/` 和对应哈希清单。`install.py` 默认保留
Data 应用，带 `--update-apps` 才替换同 ID 更新并保留存档。`bundle-push` 可只上传单个
应用。字体使用完整内容哈希命名；设备校验后原子提交，重启生效，旧字体保留供固件回退。
首次从内嵌字体固件做无线 OTA 时，旧 system 缺少新字体，界面使用完整 20px 字体退化显示；
随后从 ZIP 安装匹配字体并重启。该资源更新流程不提供联网应用分发。

## 本地验证与发布

从仓库根目录执行，所需 Rust 版本由 `rust-toolchain.toml` 固定：

```sh
./scripts/check.sh
python3 scripts/release/package.py build --out target/release-candidate --allow-dirty
python3 scripts/release/package.py verify target/release-candidate --allow-dirty
```

`--allow-dirty` 只用于检查本地未提交改动，manifest 会记录该状态，发布器拒绝发布这类产物。
输出目录必须不存在。`--upstream-image` 可指定已下载的官方底包，仍需匹配固定的完整哈希。

正式发布先更新 workspace 版本及 Cargo.lock 中各 workspace 包的版本，保留外部依赖版本，
并添加对应的 `notes/vX.Y.Z.md`。提交后等待分支流水线通过，再创建并推送 annotated tag：

```sh
git tag -a vX.Y.Z -m 'Release vX.Y.Z'
git push origin vX.Y.Z
```

tag、workspace 版本和应用 descriptor 必须一致。已公开 Release 的重跑只校验已有附件；
附件与新构建不一致时拒绝覆盖。草稿 Release 可以补传同一 tag 的附件并重新回读验证。

发布器通过 `gh release view` 查询草稿；GitHub 的 REST 按 tag 查询接口仅返回已公开版本。
恢复中断的发布时，可以下载该 tag 工作流的 `firmware-release` artifact，独立运行发布器：

```sh
gh run download RUN_ID --name firmware-release --dir target/release-recovery
python3 scripts/release/publish.py target/release-recovery --repo umbrella22/dangoOS
```

发布器核对 manifest 中的源码提交与远程 tag。恢复使用原构建附件，tag 指向保持固定。
