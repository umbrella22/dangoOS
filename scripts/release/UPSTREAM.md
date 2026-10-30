# 官方组件来源

全量镜像中的 Mosaico Bootloader v2 和 Vibe Mode 来自 Espressif 公开提供的
[Mosaico Claw v0.6.0](https://mosaico.ai/zh/firmware-update/)，底包构建日期为 2026-10-23：

- 下载地址：<https://mosaico.ai/api/v1/firmware/2/download>
- 底包 SHA-256：`bd17cec9b0ef166f29549da8ecd8163fb0181a6908d7bba654ff75b250a0bbba`
- BL：`mosaico_boot_v2`，构建时间 `Oct 21 2026 19:45:22`。
- BL SHA-256：`142bd570559b3e60989b1b97b1aec692870b6f60d8a3006230086dc8e1f3edf6`。

这些组件按固定偏移提取并保持字节不变，来源与组件哈希也记录在 `manifest.json`。
底包的工厂 UI 资源仍参与来源校验，但不装入当前双 6 MiB 布局；其原地址属于应用槽。
官方组件的版权及适用条款属于各原作者；dangoOS 的版本号不替代官方组件版本。
项目的 system 应用由仓库源码重新打包，各应用的来源与许可证位于全量 ZIP 的
`application-docs/`。运行依赖的许可证和本地修改说明随 GitHub 发布的源码归档提供。

全量镜像的分区表、OTA 元数据、dangoOS 应用和 system 镜像由本项目生成。
NOR 的 NVS 和崩溃记录使用擦除状态，不包含设备备份内容。NAND 用户卷独立于 NOR
全量镜像，通过显式初始化和正常文件系统操作管理。
