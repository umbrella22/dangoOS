# dangoOS 构建与安装制品

`S31ONE03` 使用独立 Recovery 与一个 12 MiB 主应用分区。主系统下载更新至 NAND，关闭并校验文件后写入 `Staged` 启动记录；Recovery 完整预检暂存文件，再替换主应用、完整读回并提交 `Trial`。试启动持续健康 30 秒后，主系统提交 `Ready`。安装中断、试启动失败或人工救援请求进入 Recovery。

| 组件 | 来源与验证 |
| --- | --- |
| 自有引导程序 | `bootloader/`，固定 `idf-revision.txt`；构建回执绑定源码、二进制和 IDF 修订 |
| Recovery | `crates/recovery`，USB 只读导出、显式重装、独立显示触摸与内置动画 |
| 主应用 | `crates/os`，ESP 镜像校验、S31ONE03 标记、芯片与应用描述符 |
| 分区表 | `partitions.csv`，与 `boot-contract`、主机布局定义逐字段核对，包含 MD5 |
| system | `app-pack system` 构建并重挂载验证的只读资源卷 |
| NAND 资源 | 独立应用包、UI 字体和团子；包清单、SHA256、CRC 和格式校验 |

主应用 BIN 通过 Recovery USB 或 NAND 暂存安装，更新范围限于主应用和启动记录。全量 BIN 从 NOR 地址 0 写入，包含自有引导、Recovery、新分区、初始 Trial、主应用与 system；其余 NOR 字节填充 `0xff`。全量安装重置 NOR 配置，独立 NAND 不在 NOR 写入范围。

两个 ZIP 分别包含 `-main.bin` 或 `-full.bin`，并带有 `install.py`、`tools/console.py`、`tools/recovery.py`、应用、字体和团子资源。`install.py` 在开启端口前检查包清单。资源经暂存、完整校验和同卷提交；已挂载 NAND 不会被格式化。仅 `--format-nand` 可显式初始化无法挂载的新卷或可丢弃卷。

## 本地构建

```sh
cargo fw --locked --offline
cargo recovery --locked --offline
python3 scripts/release/build_bootloader.py \
  --idf /path/to/pinned/esp-idf --tools /path/to/idf-tools --build target/bootloader
python3 scripts/release/package.py build --out target/release-candidate \
  --bootloader target/bootloader/bootloader/bootloader.bin --allow-dirty
python3 scripts/release/package.py verify target/release-candidate --allow-dirty
```

发布制品要求干净源码，省略 `--allow-dirty`。`package.py` 要求与当前引导源码相符的构建回执，导出并校验主应用和 Recovery，构建 system、应用包和字体包，再生成两个 ZIP、manifest、安装说明及 `SHA256SUMS`。打包器独立重建每个 NOR 区域并逐字节检查全量镜像，检查 ZIP 清单、资源格式和所有哈希。

`manifest.json` schema 2 记录源码提交、dirty 状态、S31ONE03、各区域偏移、引导构建来源、应用描述符、资源与链接预算。镜像 SHA256 证明传输完整性；当前制品没有发布者数字签名。`hardware_validation` 单独记录设备验收，构建成功不能替代设备启动、安装中断和数据保留证据。

## 发布流程

固件版本与 tag 相符，发布说明位于 `notes/vX.Y.Z.md`。流水线构建固定 IDF 引导和 Recovery，执行工作区、设备交叉编译、应用、驱动、Python 和预算检查后保存制品。tag 发布任务先创建或复用 draft，上传、下载并再次校验，最后发布。重复执行不会绕过制品与 tag 提交一致性检查。

```sh
python3 scripts/release/publish.py target/downloaded-release --repo umbrella22/dangoOS
```

发布命令修改 GitHub；本地构建与验证不会创建提交、tag 或发布。
