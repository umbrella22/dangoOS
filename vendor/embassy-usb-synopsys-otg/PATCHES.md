# Synopsys OTG 本地补丁

此目录保存此前 ESP32-S31 固件实际编译的 `embassy-usb-synopsys-otg` 0.4.0 源码。`LOCAL_PROVENANCE.json` 记录每个原始缓存文件的哈希和上游元数据提交 `664d4ead36bb24a63955ca649bcec66c6e70bf6d`。共享 Cargo 缓存未修改；根目录 `[patch.crates-io]` 使依赖可从仓库复现。

缓存源码已包含启用非 EP0 OUT 端点时的预备接收，以及 IN 启用时的 SNAK 和 FIFO 刷新行为，这些修改保留。除 `src/lib.rs` 外，其他原始缓存源码文件逐字节不变；新增 `src/reset_in.rs` 提供关停状态机和宿主测试。README、CHANGELOG 和本说明已中文化，原始来源 JSON 哈希仍描述导入时文件，不用译文覆盖。MIT 与 Apache 许可证原文来自上述上游提交。

## USB 复位重新配置前停止 IN 传输

一次重连采集显示：应用 TX 任务处于 `TxWait` 时已到达 `configured(true)`，bulk IN 的 EPENA 仍置位并保留旧传输状态；后续写入即使 FIFO 空间足够也持续等待。这支持“旧 IN 激活状态跨越复位重新配置”的假设，该候选变更的硬件验收在此记录中仍待完成。

收到 USBRST 后，总线在原有 FIFO 刷新和端点配置前，对已分配的非控制 IN 端点执行：

1. 设置 SNAK，等待持续的 `DIEPCTL.NAKSTS`。
2. 在短临界区重新检查 EPENA；自然完成的传输跳过 EPDIS 并返回 Idle。
3. 仅清除过期 EPDISD 确认，请求 SNAK/EPDIS，再按原禁用顺序清除 USBAEP。
4. 等待 EPENA 清除，且出现原始 `DIEPINT.EPDISD` 或 IRQ 锁存的 EPDISD 确认。
5. 所有受影响 IN 端点完成后，才执行原有 `init_fifo()` 和 `configure_endpoints()`。

IRQ 处理器在原有 W1C 确认前记录 EPDISD。轮询同时检查原始状态，因为原中断掩码未启用 EPDISD；被消费或屏蔽的中断不能成为唯一完成信号。

每次硬件等待返回 `Poll::Pending` 并自行唤醒，没有新增自旋循环或长临界区等待。没有超时或回退：缺少硬件确认可能让 USB 复位/枚举一直等待并消耗轮询时间，其他执行器任务仍可调度。现有 FIFO 刷新等待不变。

EP0、OUT 处理、正常 `write()`、SET_CONFIGURATION、数据包内容、VBUS/DTR 策略、重试及 OTA 行为不变。硬件验证检查重连后立即恢复命令，以及配置时 IN 端点处于非活动状态。EPENA 清除且后续写入正常覆盖时，旧 DIEPTSIZ 值本身不能证明失败。宿主端口关闭仍可能使普通 IN 传输等待。

## 参考与验证

- [Linux v6.12 DWC2 的 `dwc2_hsotg_ep_stop_xfr`](https://github.com/torvalds/linux/blob/v6.12/drivers/usb/dwc2/gadget.c#L3914)：NAK 生效、EPDIS、Endpoint Disabled，之后刷新 FIFO。本项目未导入其超时/回退策略。
- 原驱动的等时 IN 规避路径也先等待 NAK 生效和 EPDISD。
- 固定版本 ESP32-S31 PAC 将 NAKSTS 描述为核心正在发送 NAK，要求请求 EPDIS 前 EPENA 已置位，并等待 Endpoint Disabled 确认。EPENA 先于 Endpoint Disabled 或 Transfer Complete 清除。

两个纯状态机宿主测试覆盖所需顺序、未完整确认、自然完成和重复轮询。默认及 v3 诊断固件构建、镜像验证和 ROM 检查通过，设备重连验收仍单独要求。冻结前后源码、缓存到 vendor 的差异、参考和两组镜像保存在 `target/usb-in-reset-fix-20261024`。
