# N4 常驻 Lua 大脑现场记录

设备为 CoreBoard V1.2，交互板在右槽，仍使用旧分区布局。所有更新仅写
`main_app` 的 `0x210000` 起始区域；未执行 N6 迁移。

## 热加载、拒绝候选与持久化

第一轮运行普通镜像 `7b7b65b0c5575e6734b332f0804ba3065c602cecac8f349b97e7416535d20fe7`。
证据位于 `target/n4-live-20261024/`，根审查汇总为
`partial-acceptance-through-10.json`，原始采集、回执和操作者反馈分别保存。

| 用例 | 现场结果 |
| --- | --- |
| 默认基线 | Lua/default，29672 B，CRC `bc14ae2e`，error=0 |
| Happy 热加载 | commit 成功，Lua/custom，402 B，CRC `e36bbd88`，error=0；操作者确认持续开心 |
| 错误候选 | 97 B 缺少闭合 `}` 的脚本被 `brain-commit` 拒绝；原 Happy 指纹和状态保持，操作者确认开心及点按有效 |
| Happy 重启保留 | 一次软件重启，主机观察到 USB 消失和返回，启动日志为 CoreSw；存储挂载后加载相同 Happy，操作者确认稳定后的表情与点按 |
| 延迟运行时错误 | 新脚本先 commit 成功、custom/error=0；约 10 秒后为 native/custom/相同源码 CRC/error=1，绘制持续，Happy 转 Idle |
| 原生接管后的反应 | 后续日志有 Peering、Satisfied、Puzzled、Angry 和 Angry 震动请求；操作者反馈画面无异常。没有独立触感或精确触摸延迟结论 |
| 恢复默认 | `brain-default` 成功，随后精确 default/29672 B/`bc14ae2e`/error=0；皮肤仍 default，下载应用仍为原四个 |

延迟错误的严格用例 `08-delayed-error` 保留为 **failed**：状态回退及通信存活
通过，但原始设备日志没有错误原因，无法完成故障分类。主机真实 VM 测试支持夹具
预期，不能替代设备原因日志。未在该镜像继续测试循环或内存耗尽。

```mermaid
flowchart LR
    A[提交候选] --> B{初始化与首次 tick}
    B -->|失败| C[保留原 VM 与持久脚本]
    B -->|成功且持久化成功| D[新 Lua 大脑运行]
    D -->|后续回调失败| E[释放失败 VM并启用原生行为]
    E --> F[保留故障源码指纹和错误状态]
    F -->|显式 brain-default| G[删除持久覆盖并恢复默认 Lua]
    style C fill:#c8e6c9,color:#1a5e20
    style E fill:#fff3e0,color:#8a4300
    style G fill:#bbdefb,color:#0d47a1
```

## 运行时故障原因日志修复

源码核查确认 `Controller::dispatch_inbox` 原来在失败时仅保存错误、释放活动 VM，
普通运行时回退没有打印原因。启动失败和命令拒绝已有日志，不能覆盖这一分支。

`02ce70d` 在唯一活动 VM → inactive 转移处增加一次
`brain: runtime fallback: ...` 警告。失败 VM 先释放；完整错误继续保存在 Controller，
日志复用 96 B 首行格式化，阻止脚本换行伪造协议回执。后续 inactive 调用不重复打印，
有效替换后的新故障可以再次报告。回调预算、候选事务和持久化规则保持原语义。

`target/n4-runtime-fallback-log-20261024/` 保存主机与构建证据：269 项库测试、
真实 VM 的普通错误／预算耗尽／内存不足分类、故障后 100 次调用不刷日志、重新加载
后再次报告、事件错误的换行截断、普通固件构建、严格镜像检查及 714 个 ROM 符号
检查通过。根代理独立核对了 586 个源码指纹和产物。该目录的 `6609b356…` 镜像未刷入；
实际刷入的是包含下一项清灯改进的组合镜像。

## RESET 后的早期清灯

`c86811b` 将原 RMT 灯光／振动初始化提前到 USB task 启动之后、LCD／触摸／UI／
模块初始化之前。启动最多让出等待 100 ms，确认六颗 WS2812 黑帧发送及至少
280 μs 锁存完成；无回执时记录警告并继续启动。原电源 GPIO57／GPIO60 配置未改变。
9 项灯光／按钮生命周期检查、普通构建、严格镜像和 ROM 符号检查通过。

此路径在应用开始运行后执行。按住 RESET 或停留 ROM 下载模式时应用不运行，
不能据此声称复位边沿熄灯、下载期间断电或全部外设断电。V1.2 的 GPIO60 控制
屏幕和外扩供电，板载外设还有其他电源域；资料及电气边界见
[V1.2 资料核对](lessons/esp-mosaico-v1.2-reference.md)。

组合镜像位于 `target/lights-early-reset-clear-usb-first-20261024/`：

- BIN：3,057,568 B；SHA256 `c8e4f00de7a31aec5936ffc3f54cedd0b0fc992a595beee07949365f54f9af42`。
- ELF：SHA256 `1032e91beaa194396a2ff4d17ee5f2abeb76a2b56251e9cc9409e2d56dd2aec7`。
- `target/usb-session-live-20261024/flash-06/`：完整应用读回一致、引导区与原备份
  逐字节一致、历史应用头一致；随后一次 watchdog-reset。
- `27-n4-early-lights-startup`：默认脑与皮肤查询正常，启动黑帧成功回执位于触摸
  初始化之前。该日志顺序不等于 RESET 边沿到物理熄灯的测量。

`target/reset-peripheral-review-20261024/03-physical-reset` 观察到 USB 消失和返回，
随后完成启动及默认脑、默认皮肤查询。黑帧完成位于日志第 19 行，LCD 清屏从第 20 行
开始；没有清灯超时警告。操作者反馈「主页恢复以后灯光熄灭，点击有效」。这确认了
重启后的残灯清除和触摸恢复，未确认物理熄灯早于主页或等于 RESET 边沿。
`04-cleanup` 收到临时测试页 `rmdata`、`rm` 成功回执，下载列表精确恢复原四个应用，
默认脑与默认皮肤保留。未将管理回执扩大为删除后 appdata 路径的直接检查。

## 组合镜像故障复测

组合镜像的新验收目录为 `target/n4-reset-live-20261024/`，通过 `image-preflight.json`
绑定实际 flash-06、启动、物理 RESET 与清理后的默认基线。`acceptance-through-10.json`
独立核对了前 10 阶段的原始文件指纹，旧目录的 `08-delayed-error` failed 保留不变。

| 新镜像用例 | 设备证据 |
| --- | --- |
| Happy 与语法拒绝复测 | 402 B／CRC `e36bbd88` 激活；错误候选拒绝后仍为同一 Happy，error=0 |
| 延迟普通错误 | 230 B／CRC `1b34eac5`，先 Lua/error=0，约 10 秒后记录 `N4 delayed ordinary error`，变为 native/error=1 |
| 延迟无限循环 | 213 B／CRC `30cf7e3d`，先 Lua/error=0，随后记录 `callback budget exhausted (wall; ...)`，变为 native/error=1 |
| 延迟内存耗尽 | 248 B／CRC `d80693d8`，先 Lua/error=0，随后记录 `not enough memory`，变为 native/error=1 |

三种故障的原因均在对应采集窗口内只出现一次，回退后 hello、精确状态查询和 plane
帧遥测继续。前两种故障后显式恢复默认，再安装 Happy 作为下一轮起点；没有自动
重复执行失败脚本。以上不替代每种故障后的物理关机或长期稳定性验收。

`11-native-after-fault` 在内存故障后的原生状态采集约 596 秒，操作者反馈正常。
日志记录了触摸按下／松开、Satisfied／Puzzled 表情与相应震动请求；该窗口没有
单独捕获 Angry，因此不另列一次精确 Angry 对照。随后 `12-default-after-oom`
显式恢复默认脑，原四个下载应用和 default 皮肤仍在。

## 前台共存与默认恢复

`14` 至 `16` 安装并运行唯一只读前台页 `n4_readonly_20261024_c7a91e`，后台使用
20 秒后报错的脑（229 B／CRC `a7815675`）。单次采集先确认 Lua/error=0，取得前台
Alive 0～35 的全部 36 个顺序记录；后台在 Alive 17 后报告一次具体错误，前台继续
到 35 并正常退出，末尾查询为 native/error=1。操作者反馈「无异常」。宿主收到的
相邻 Alive 最大间隔为 1.069 秒以内，此数值不作为设备显示延迟测量。

前台页开始和结束均检查自己的空 appdata 根；持久 active.lua 与后台源逐字节匹配，
`.incoming` 和 `.next.lua` 均由 exists=false 加 stat ENOENT 证明不存在。`17` 显式
恢复默认、删除前台页及其数据；`18` 至 `19` 重新安装 default 模式只读检查页，
直接确认 active.lua、`.incoming`、`.next.lua` 在开始和结束时全部不存在。

`20` 再次 stop、rmdata、rm 并取得成功回执，应用列表精确恢复原四个，default 脑与
皮肤保留，存储空闲回到基线 463 个 data 块。最后 appdata 目录删除仅有管理回执，
未额外独立 stat。`21` 软件重启观察到 USB 消失／返回，重启前后均为默认 Lua 脑
29672 B／CRC `bc14ae2e`／error=0，原四应用与 default 皮肤保留。
`acceptance-through-21.json` 核对了全部 21 阶段的收据及原始采集指纹。

## 自定义输出优先级

后续 `22` 安装稳定 effects 脑（265 B／CRC `ea33a750`）：每 tick 设置 Happy 和
六灯低亮红，每隔至少 2 秒请求一次 Happy 震动，无导航或关机命令。这个源持续运行
至显式恢复默认，用于区分设置禁用和脚本自己停止。

| 阶段 | 现场结果 |
| --- | --- |
| `23` Home 基线 | 操作者确认 Happy、六红灯和周期短震动三项正常 |
| `24`／`25` 关闭设置后回 Home | 灯灭且无震动；随后仍查询到同一 effects 脑 active=lua/error=0，5 秒采集中没有震动任务日志 |
| `26` 恢复设置并返回 Home | 设置页暖白预览、启用震动时一次提示、随后无周期震动；Home 恢复 Happy／六红／周期震动，操作者确认两页面符合预期 |
| `27`／`28` 前台蓝灯页 | 六蓝灯覆盖后台红灯，Alive 持续增加，后台周期震动停止；操作者确认一致 |
| `29` 外部停止 | stop 回执及 effects 脑状态正常；前台退出到应用列表，恢复 Home 需另行导航 |
| `30`／`31` 原生 Panic | 物理晃动后两次 Panic 表情和 Panic 任务日志，之后恢复 Happy 与周期 Happy 请求；操作者确认惊慌、灯光变化及恢复。同一 effects 脑仍为 Lua/error=0 |

前台唯一 ID 为 `n4_priority_blue_20261024_b83f2a`，无文件写入。`28` 取得 Alive
0～101 的连续记录，BLUE BEGIN 后没有震动任务日志。该页及 effects 脑在 `33` 清理；
`20`／`21` 的早期清理结果不覆盖这些后来新增的内容。
Panic 任务日志为原规格 3×80 ms／100%；操作者未单独确认三段触感，不将任务记录
扩大为本轮 PWM 或触觉分段测量。

## POWER 开机后交互板识别失败

操作者在专用关机采集启动前执行 AI 关机、POWER 开机，报告关机能熄灯，重新开机
后 Happy 和周期震动保留，但灯带不亮。`32-unattended-power-led-state` 捕获启动
日志：Startup 黑帧完成、I2C1 `0x51` ACK，随后一次 `right slot: EEPROM read failed`，
后续遥测持续 `R=bad derr=1`，没有交互板激活记录。持久 effects 脑仍为原 CRC、
Lua/error=0；Happy 和周期震动符合该脚本，未恢复的灯带属于识别故障。

旧 `Subboard::scan` 仅在去抖确认插入时读取完整描述符。一次传输失败被保存为
`Unreadable`，持续探测成功不会再形成插入边沿，因此没有再次识别的机会。
`lights::set_attached` 依赖识别成功的活动右槽，未识别时不输出日常灯光。
首次传输失败的电气原因尚未建立，不能据此判定供电、EEPROM 或灯带损坏。

`33-led-bug-restore-default` 已取得 stop、brain-default、临时蓝灯页 rmdata／rm
成功回执；默认脑恢复为 29672 B／CRC `bc14ae2e`／error=0，皮肤仅 default，
下载列表精确恢复原四个，data 空闲回到 463 块。该次关机没有同步采集 AI／Sleep／
黑帧／USB 消失序列，不能填入正式并发关机通过项。

后续 `0b1b211` 普通重试修复已仅写 main_app，完整回读与引导区一致，使用独立目录
`target/slot-recovery-live-20261024/`。新目录 `02`／`03` 一轮默认脑下的 AI 关机／
POWER 开机、灯光和双键通过；此结果不替代本节未完成的自定义脑／前台应用并发关机。
两次描述符首读均成功，失败后的重试分支尚未取得真机证据，见
[识别失败复盘](lessons/subboard-descriptor-retry.md)。

## 验收边界

N4 整体尚未通过。以上完成一轮前台／后台调度、直接文件清理与默认重启，不能扩大
为长期稳定性。设置禁用、Settings 预览和前台灯带覆盖已取得本轮设备证据；全部输入
时间线和自定义脑并发关机仍须逐项取得设备证据。原生 Panic 本轮有实际触发与覆盖
证据；原生 Error 目前没有
已验证的现场注入入口，自定义脚本选择 Error 表情不能替代该项。
