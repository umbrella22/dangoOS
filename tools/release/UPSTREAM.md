# 引导与资源来源

`S31ONE03` 全量镜像使用仓库 `firmware/boot/loader/` 中的启动选择实现和固定修订的
[ESP-IDF](https://github.com/espressif/esp-idf)。修订记录于
`firmware/boot/loader/idf-revision.txt`；构建回执绑定该修订、引导源码集合和输出二进制的
SHA-256。ESP-IDF 提供芯片初始化、分区解析和 ESP 镜像加载；本项目定义
Recovery、单主应用、启动记录和试启动选择。

Recovery 由 `firmware/boot/recovery` 构建。它复用 `firmware/platform/s31` 的板级驱动，
具有独立 USB、显示、触摸和只读存储路径。动画随 Recovery 镜像交付，
无需读取主应用或 NAND。当前全量包不包含官方 Mosaico Bootloader 或 Vibe Mode。

分区表、初始 `Trial` 记录、主应用和 system 镜像均由本项目生成。NOR 的
NVS 和崩溃记录区域初始为擦除状态；全量包不包含设备备份或运行期配置。
NAND 用户卷独立于 NOR 镜像，由资源安装器与正常文件系统操作管理。

system 应用从仓库源码重新打包，各应用的来源与许可证位于全量 ZIP 的
`application-docs/`。运行依赖的许可证及本地修改说明保留在源码中。
ESP-IDF 和其他第三方组件的版权及适用条款归各原作者。
