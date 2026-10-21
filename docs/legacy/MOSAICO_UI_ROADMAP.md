# 官方 UI 重写路线图（Slint 版 Mosaic UI）

> 目标：按官方 esp-mosaico-claw 的 UI 设计（`docs/MOSAICO_UI_DESIGN.md` + `docs/mosaico-ui-preview.html`）
> 用 Slint 重写固件界面。范围拍板（2026-09-30）：不做 AI 页；天气走官方 API；
> IMU/TOF/相机/打砖块全做（模块都有）；风扇不设专页、只做一张官方风格数据卡。
> 状态标记：✅ 完成 / ◑ 部分完成 / 🔨 进行中 / ⬜ 未开始。
> 2026-09-30 审查修订：M2 仍缺动态指针；M3 的 TOF/NAND 驱动仍待接入。
> 触摸和亮度已通过板端确认；新增手势、布局及左槽相机预览通过 50 项软件回归，修复包已刷入并通过设备摘要校验，尚待新版固件板端验收。
> 详细问题和验证结果见 [MOSAICO_UI_REVIEW.md](MOSAICO_UI_REVIEW.md)。

## 架构决策（已定）

| 决策 | 内容 |
|---|---|
| UI 编译分流 | `build.rs` 按消费者选根：桌面(std)=旧 fan_control.slint；固件(embedded)=`ui/mosaic/mosaic.slint`。**两套 UI 严禁同入固件**（.text 翻倍，ROM 溢出 420KB 实测） |
| 字体 | 官方 NotoSansSC 子集化（fonttools，ASCII+中文 UI 词表）= 119KB；官方原版 1.1MB 不可用。家庭名 "Noto Sans SC" |
| 指针渲染 | Slint 软渲染器不支持 Rectangle 运行时旋转（仅 Image 90° 步进）→ M1 时钟用静态 Path 多边形（10:10 表位）；动态指针 M2 由 Rust 在 present() 前直接画帧缓冲 |
| 旧 UI 去留 | `platform.rs`（fan UI 固件层）已删；`fan_control.slint`+`boot.rs`+`history.rs` 留给桌面模拟器，迁移完成后移除 |

## M1 ✅ Hub 骨架上屏（2026-09-30，v25 交付包）

- ✅ `ui/mosaic/`：palette.slint（官方 tokens）+ status-bar.slint（时间/WiFi 五档/电池三态）+ hub-home.slint（天气卡 95px + Atelier 时钟 + 3 快捷）+ hub-apps.slint（3×2 应用槽）+ mosaic.slint（PageFlow + 页点 + Home 按钮）
- ✅ 官方资产入库：16 张图标 PNG（Apache-2.0）+ 子集字体
- ✅ `platform_mosaic.rs`：触摸→Slint 事件、uptime 时钟、心跳（渲染耗时/DMA 错误/触摸计数）
- ✅ 双端零警告，release 2.24MB，v25 交付包（app 1.56MB/8MB 分区）
- 坐标来源：官方场景 JSON 逐像素（hub-home 布局 = launcher_flow_tab0 原值）

## M2 ◑ Hub 交互（2026-09-30，动态指针待完成）

- ✅ 下拉面板 Drawer（顶部点击/下拉及 Settings 亮度行可打开；上滑收起；音量/亮度滑杆按起手位置持续接收拖动；关闭手势不触发底层页面；上滑待新版固件板端验收）
- ✅ Home/Apps 横滑（按方向和位移在释放时切页；图标、Home 按钮、页点起手均覆盖；通过输入回归，待板端验收）
- ✅ Hub 官方布局父层修正（内容保留顶部 56px，天气/时钟坐标按 tab 计算，表针采用官方深色；软件截图通过）
- ✅ 锁屏 Lock Screen（144px 大时钟/日期/CHRG；上滑 60px 解锁，点击保持锁定；AI 键 GPIO7 切换）
- ✅ Settings 页框架（列表行 456×68 r20 + 风扇数据卡 152px + 亮度/WLAN/About 行）
- ⬜ 动态时钟指针（待做：Rust 画帧缓冲）
- ✅ 亮度接入显示驱动（启动和数值变化写 `0x51`，失败重试；触摸地址修复后，实际调节已通过板端确认）

## M3 ◑ 应用页与驱动（2026-09-30，模块功能部分完成）

- ✅ 共享 I2C 总线（shared_i2c.rs：critical-section Mutex，触摸+IMU 同挂 I2C0）
- ✅ BMI270 驱动代码（Bosch 8192B 配置装载/初始化状态校验/±2g@100Hz/`0x0C` 16bit→mg；配置带来源和许可）+ IMU 页（气泡居中，无有效样本显示 "--"；读数待板端验收）
- ✅ TOF 页（模块接入前显示 "--"/WAIT；阈值按完整滑杆映射 0–100，并保存根状态）
- ◑ 左槽相机预览（EEPROM/传感器 ID 检测，OV3640 RGB565_LE / SC101IOT UYVY，独立 AXI DMA 接收，完整帧校验、超时与重试；软件回归通过，实际采集待板端验收。照片保存未实现，快门禁用）
- ✅ Breakout 打砖块（单调时间 30Hz：触摸挡板/反弹/24 砖/3 命/计分/结束后重开；隐藏或被覆盖时暂停，解锁触点不影响挡板）
- ✅ Album 相册占位（九宫格 + NAND 空提示；SPI NAND 驱动接入后填充）
- ✅ Weather 页（官方布局+5 张插画映射；数据待 M4）
- ✅ 页面导航统一（Apps：Settings/IMU/Album/TOF/Bricks/Weather；Home：Camera/Weather/Album；应用页独立返回，不叠加 Hub 导航）
- ⬜ TOF 驱动及实测、相机板端采集验收、NAND 相册存储驱动
- ◑ 扩展槽识别与 GPIO 仲裁；相机接入前校验左槽 EEPROM，默认不启用复用 GPIO12 的风扇 PWM，显式 feature 禁用相机并仅初始化零目标风扇通道。通用模块发现与仲裁仍待实现

## M4 ◑ 天气/网络（2026-09-30 部分完成, 受阻于生态）

- ✅ Weather 页 UI 全量（app-weather.slint，官方逐像素）
- ❌ **WiFi 阻塞：Rust 生态 S31 尚无 esp-wifi-sys**——esp-radio(rev bb239a0f) 的芯片表/esp-wifi-sys 依赖列表完全没有 esp32s31 条目（官方固件的 WiFi 走 IDF v6.2-dev C 侧）。已验证并回滚依赖。**解锁条件：esp-wifi-sys 发布 s31 包 + esp-radio 接入**（跟踪 esp-rs/esp-wifi-sys 与 esp-hal main 分支）
- ⬜ WiFi 解锁后：esp-rtos 混合模式（阻塞主循环+embassy 任务）→ 配网 → 官方天气 API（claw 源码 weather 服务）→ NTP 真实时间 → 状态栏/锁屏日期喂真实值
- ⬜ 天气数据接通后：weather-temp/desc/city/art 属性已全部就位，platform 层直接喂

## M5 ⬜ Lua 工具层（另行立项，见记忆「Lua 热重载立项评估」）

## 交付状态

| 里程碑 | 交付包 | ELF | app.bin |
|---|---|---|---|
| M1 | v25 | 2.24MB | 1.56MB |
| M2 | （增量） | 3.58MB | — |
| M3+M4 | **v27** | 4.81MB | 3.56MB/8MB |

## 已知坑（勿踩）

1. **双 UI 同编 → .text 溢出 420KB**（ROM 4MB 预算容不下两套 Slint 组件树）
2. **Slint Rectangle 无 rotation-angle**（软渲染器限制）；Image 仅 90° 步进
3. **官方原版字体 1.1MB 直接入库必溢出**；必须 fonttools 子集化（词表在 `docs/MOSAICO_UI_DESIGN.md` 提取脚本）
4. 旧 `platform.rs` 已删——桌面模拟器仍走 fan_control.slint（std feature 门控）
