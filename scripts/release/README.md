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
| `publish.py` | 验证远程 tag 与源码提交，上传并回读附件，再公开 Release |
| `notes/` | 各 tag 的发布说明 |

## 镜像边界

OTA 包只含应用镜像，通过运行时控制台写入非活动槽。它不更新 BL、分区表或 system。
全量包从固定哈希的公开官方底包提取 BL、Vibe Mode 和工厂 UI 资源，使用仓库源码构建
system，填充 A/B 分区与 slot 0 应用。初始 OTA 记录为 `sequence=1, state=Valid`。
其余空间以 `0xff` 初始化，包含空白 slot 1、用户 data、NVS 和崩溃记录。

打包过程校验芯片、应用版本、A/B 标记、应用校验和、分区表 MD5、各组件偏移与边界、
system 文件树和全部附件 SHA-256。全量镜像固定 16 MiB，从地址 0x0 写入，会清除用户数据。
实际设备上的首装、BL 和恢复路径验收单独记录；构建通过不替代硬件验收。

## 本地验证与发布

从仓库根目录执行，所需 Rust 版本由 `rust-toolchain.toml` 固定：

```sh
./scripts/check.sh
python3 scripts/release/package.py build --out target/release-candidate --allow-dirty
python3 scripts/release/package.py verify target/release-candidate --allow-dirty
```

`--allow-dirty` 只用于检查本地未提交改动，manifest 会记录该状态，发布器拒绝发布这类产物。
输出目录必须不存在。`--upstream-image` 可指定已下载的官方底包，仍需匹配固定的完整哈希。

正式发布先更新 workspace 版本及 Cargo.lock 中四个本地包的版本，保留外部依赖版本，
并添加对应的 `notes/vX.Y.Z.md`。提交后等待分支流水线通过，再创建并推送 annotated tag：

```sh
git tag -a vX.Y.Z -m 'Release vX.Y.Z'
git push origin vX.Y.Z
```

tag、workspace 版本和应用 descriptor 必须一致。已公开 Release 的重跑只校验已有附件；
附件与新构建不一致时拒绝覆盖。草稿 Release 可以补传同一 tag 的附件并重新回读验证。
