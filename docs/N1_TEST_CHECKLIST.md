# N1 集中真机验收清单

状态日期：2026-10-24（北京时间）。范围为六个 Lua 硬件模块、资源释放、马达诊断和
共享传感器采样的性能影响。N1 尚未整体验收通过：Angry 同预设原生 / Lua 对照通过，
正式默认镜像的 Angry/Happy/Error/Panic、左右快速双击和震动设置功能已经复测通过；
时钟修复后普通镜像 n=65 的触摸 P95=51ms（此前74ms），仍未达到50ms门槛。
实际 PWM / 执行中取消时序仍待测。

设备当前 BIN、SHA-256 与 feature 状态见
[现场镜像身份](N1_HAPTIC_LIVE_20261023.md#当前运行镜像)。实验镜像的触感不等于正常预设通过。
旧布局 main_app 完整读回与引导区前后比对通过，未做 N6 迁移。最新现场证据见
[N1_HAPTIC_LIVE_20261023.md](N1_HAPTIC_LIVE_20261023.md)，原始日志在
`target/n1-live-20261023-143145-nmf7roas/`。

历史 N1 默认 / 诊断镜像仅用于旧布局复现，指纹保存在 LUA_HARDWARE.md。当前正式默认 / 诊断镜像见 [现场记录](N1_HAPTIC_LIVE_20261023.md#当前运行镜像)，
此前取消修复见 [测试前核对](lessons/n1-n7-preflight.md)。N6 迁移后
不得刷回 N1 历史镜像，因为其 data 定位会进入新的应用槽；使用 [N6 流程](N6_MIGRATION.md) 更新或恢复。

## 待测项目与顺序

优先完成 1–4，再在默认固件上完成 5。常规观察约 10–15 分钟；波形测量另计。
勾选表示取得对应证据，不能仅凭串口请求成功勾选实物项目。

| 完成 | 顺序 / 项目 | 操作与通过条件 | 当前缺口 |
| --- | --- | --- | --- |
| [ ] | 1. 同预设马达对照 | 刷机前先记录原生 Angry → Lua Angry 四次 → 原生 Angry；需要诊断时换当前集成诊断版，重做全部三段。记录触感与日志，检查关闭 / 开启取消语义 | 旧镜像原生 / Lua / 原生通过；正式镜像四种预设、主页 Angry、关闭拒绝与开启恢复通过；执行中取消和电气时序待测 |
| [x] | 2. 异常退出退灯 | 运行 `n1_error_probe`：两端红/绿约 3 秒，出现 `N1_EXPECTED_FAILURE` 后回应用列表，两灯熄灭 | 集成诊断版 11 日志及操作者确认先亮后自动熄灭 |
| [ ] | 3. 异常后重开、双键、正常退灯 | 紧接着运行 `n1_contract_probe`；红/绿重新点亮，左右分别单击、双击、长按约 2 秒并松手；同步核对 down/up 边沿，结束后灯熄灭 | 12 日志及观察确认重开、正常退灯；30 日志确认去重滤波后左 8 / 右 4 双击及各一次约 2 秒长按，手感获认可；39 日志补齐普通镜像左右各 3 次双击及各 2 次完整长按，操作者确认正常 |
| [ ] | 4. 外部停止释放 | 再运行有灯的探针，执行 `stop`；返回列表，灯熄灭；再次运行时可以重新打开 LED / 两键 | 13/14 日志及观察确认外部停止熄灭；停止后 LED 再开仍需复验 |
| [ ] | 5. 默认固件稳定性与屏幕触摸 | 恢复默认固件后，在狐狸主页采样至少 3 分钟，做至少 20 次独立屏幕点按；记录帧率、丢帧、触摸统计、IMU / BMM 错误计数增量和 USB 状态 | 74 日志约210秒、n65/P50=42/P95=51/max54ms，仍未达50ms；操作者确认约60次点按与无卡顿，65组down/up；触摸后19–20fps/drop0/ierr与berr增量0，Header警告及bskip截断保留；旧47日志P95=74ms保留为基线 |
| [ ] | 6. 马达电气时序 | 测 GPIO8 PWM 与预设包络；测持续请求时相邻 pattern 启动间隔不小于 150 ms | 未测物理波形；请求日志与 `hd` 快照均不能替代仪器读数 |
| [x] | 7. 左槽按键与能力边界 | 交互板移至左槽，接口探针应使用 GPIO13 / GPIO12；两键手势可用，LED 明确返回不可用 | 54/55 日志确认左槽识别、GPIO13/12 手势、非活动GPIO11/10拒绝及LED不可用；操作者确认正常，精确漏计率未测 |

已确认的结果包括传感器读数、14 项接口探针、两键单击/长按、右键双击、初始红绿灯、
右槽拔插后旧句柄失效及新句柄恢复为双绿。若后续修复改变相关路径，应重跑对应项目；
本轮不要求仅为补左键双击而重复所有拔插操作。

## 准备与证据目录

在仓库根目录执行以下命令。交互板插右槽，USB 保持连接，设置中开启震动。
停止其他串口监视器；每次仅允许一个进程占用运行口。

```sh
cd /Volumes/project_home/github/esp-32
N1_PY=/opt/homebrew/Cellar/esptool/5.6.0/libexec/bin/python
N1_RUNTIME=/dev/cu.usbmodemmosaico_s31_r11
N1_DOWNLOAD=/dev/cu.usbmodem113401
N1_LOG_DIR=$(mktemp -d "$PWD/target/n1-retest-XXXXXX")
echo "$N1_LOG_DIR"
"$N1_PY" scripts/n1-device-session.py --help
```

端口名以当次枚举为准，可用 `ls /dev/cu.usbmodem*` 核对。本机普通 `python3` 尚未安装
pyserial；上面的 esptool Python 已具备该依赖。工具拒绝覆盖已有日志，重复某一步时使用
新的日志名。`--seconds` 是命令 / 上传完成后的采集窗口；`--wait` 仅限制等待端口的时间，
单次写入限 20 秒，协议回复限 20 / 30 秒。结束采集不会停止板上的 Lua 应用。

四个历史探针已保存到 `examples/lua-apps/n1_*_probe/`，与先前实际运行版本逐字节一致；
另有新准备的 `n1_haptic_rate_probe`，供后续物理限速测量。
详见 [N1_PROBES.md](../examples/lua-apps/N1_PROBES.md)。运行工具退出 0 仅表示传输完成，
仍需检查 Lua 汇总、预期退出方式和实物观察。

## 1. 同一固件的原生 / Lua / 原生 Angry 对照

设置开启震动；回 Home，使用原生 / 默认大脑，同一供电方式，手持设备。先记录运行
固件身份、最近烧录 BIN 的 SHA 和对应 ELF；如果无法确认，就标记身份未证实，不把
此次观察与某个归档镜像绑定。三段之间不刷机、不改震动设置。

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/native-angry-before.log" --wait 0 --seconds 40 --command status
```

在第一段采集窗口内以原来能触发生气的动作触发 Angry，核对任务确实记录 Angry，记录
「无 / 微弱 / 清晰」及能否分辨两段。冷却后运行 Lua 对照：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/lua-angry.log" --wait 0 --seconds 30 \
  --command status --push examples/lua-apps/n1_haptic_probe
```

脚本先等 5 秒，再发四次 `angry`，每次请求间隔 3 秒，每个预设有两段 160 ms、90%
占空比的脉冲。分别记录每次触感，不靠放在桌上是否有声音判断。然后返回 Home，待
生气状态和冷却恢复，再以相同动作触发原生 Angry：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/native-angry-after.log" --wait 0 --seconds 40 --command status
```

原生与 Lua 的 Angry 经相同任务和 RMT 输出。若三段不同，保留差异；若 Angry 全部
通过，再独立测 Error，不能把两个预设的触感差异直接归因于 Lua。当前设备的旧版对照
与后续集成修复版的验收分别记录。

### 按需启用诊断版

马达诊断归档为 `target/haptic-fixed-20261023/default.bin` 与 `diagnostics.bin`，
后者仅开启 `haptic-diagnostics`。触摸分段诊断及后续修复使用独立归档，不覆盖马达
诊断文件；当前设备镜像和 feature 见 [现场记录](N1_HAPTIC_LIVE_20261023.md#当前运行镜像)。
先核对 BIN / ELF、源码收据与小于 3 MiB 的容量约束。**只在已确认仍为旧布局时**
写入 main_app `0x210000`；迁移后按 N6 流程选槽。

本机验证过的流程是：人工 BOOT/RESET 进入下载模式，单个 ROM/stub 句柄核对芯片、
MAC、旧分区和当前应用头，保存引导区；只写 main_app 后完整回读应用，再比对写前 / 写后 /
原始引导区；启动运行口采集器，最后在原 stub 句柄执行一次 `watchdog-reset`。
日志 `36-fixed-default-autoboot.log` 证明正式镜像自动回到应用，无需人工再次 RESET。
`single_handle_fixed_auto_flash.py` 是绑定当时旧镜像头、目标 hash 和唯一输出路径的现场脚本，
不能直接重复运行；每次刷写必须重新核对这些输入并使用新证据目录。单独的 esptool
`Hash of data verified` 不代替上述完整回读与保护区核对。

诊断最多记录启动后前 10 个 pattern，每个只记录首段 burst，启动插板反馈也占用额度。
若没有 `hd` 行，先核对烧录指纹及额度；需要重新复位后尽早运行探针，再保存新日志。

集成修复版还需验证：关闭震动会取消当前与排队输出，原生 / Lua 新请求均受阻；再开启
仅播放开启提示和新请求，不复活旧队列。主机策略测试不替代实际 burst / gap / 限速
等待中的取消时序测量。OTA begin 的静默回执在 N6 单独验证。

寄存器解释供分析日志使用：

| 字段 | 解码与用途 |
| --- | --- |
| `p`、`b`、阶段、`t` | pattern / burst 序号、`start/prestop/stop`、单调毫秒；用来配对首段脉冲的三次观测 |
| `c` | RMT ch0 CONF0：bit3 连续发送、bit4 wrap、bit5 空闲电平、bit6 空闲输出使能、bit7 stop、bits8–15 分频、bits16–19 内存块数、bit21 carrier |
| `s` | TX_STATUS：bits0–9 读指针、bits11–20 APB 写指针、bits22–24 FSM 原始值、bit25 mem_empty、bit26 APB 写错误 |
| `i` | INT_RAW：bit0 ch0 TX end、bit4 ch0 TX error；保留其他位，不能把别的通道中断算到马达 |
| `oe8`、`m`、`o` | GPIO8 输出使能、IO_MUX、输出 matrix 原始值；比较三阶段及插板前后是否改变 |
| `tp` | 触摸 scan / output 掩码；右槽 KEY_L / KEY_R 对应 ch5 / ch4，不应占用 GPIO8 对应的 ch2 |
| `ram0` / `ram1`、`lim` | 首个 pulse 字、结束标记和 TX_LIM 原始值；按实际日志字段核对 |

这些都是顺序只读快照。中断位可能保留旧状态，FSM 值也不能凭名称猜测；结合启动日志、
调用阶段与所锁定 HAL / PAC 解码。即使寄存器看似正常，仍需 GPIO8 电气波形定位马达、
驱动电路或输出时序；当前没有已证实的故障根因。

## 2–3. 异常退灯、重开与双键

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/error-cleanup.log" --wait 0 --seconds 15 \
  --push examples/lua-apps/n1_error_probe
```

预期两端红绿灯亮约 3 秒，随后出现 `N1_EXPECTED_FAILURE` 并返回应用列表。
这次报错是测试设计，脚本刻意不主动 clear / close；观察返回列表后两灯是否熄灭，
其余灯是否仍灭。若有残灯，记录持续时间和颜色。

然后立即运行接口探针，验证异常后资源可以重开：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/contract-after-error.log" --wait 0 --seconds 90 \
  --push examples/lua-apps/n1_contract_probe
```

红/绿应再次亮起。右槽左键是 GPIO11，右键是 GPIO10；依次对两键做单击、双击、
长按约 2 秒后松手。交互板默认双击窗口已刷入 400 ms；AI / BOOT 保持 180 ms。
窗口从已防抖的松开边沿起计，第二次按下须严格早于截止；长按门槛 1500 ms。日志分别应有
`single_click`、`double_click`、`long_press_start` / `long_press_up`。重点补齐 GPIO11
的 `double_click`。不要只以总回调数判断手势齐全。

先慢做一次按下 / 松开并核对 `press_down` / `press_up`，再尝试双击。电容键约每 40 ms
采样一次，TouchKey 要连续 4 个安静样本才松开，旧实现后续 Debounce 还要 3 个同状态样本；
首个 false 已计入后级，因此理想轨迹共需 6 个安静样本，约 200–240 ms（采样相位会影响
延迟，调度也会拉长）。第二次按得过早可能根本没有第一次 `press_up`。现场逐步调整
松手间隔，以真实边沿判断。`24-fast-keys-trace.log` 已确认两段右键、一段左键在五个
quiet 样本之间被第二层去抖吞掉释放；修复直接接收 TouchKey 状态，保留四样本触摸
过滤和 GPIO 去抖。主机真实序列回放及实验镜像惯常快速双击、约 2 秒长按复测通过，
普通镜像左右各 3 次双击及完整长按已复测；未测精确漏计率。

脚本标称窗口 60 秒，既有实测约 73 秒完成，因此采集留出 90 秒。预期接口汇总 0 FAIL，
两键均有真实回调、正常结束后两灯熄灭。若窗口内没收到目标手势，记录未覆盖，不能记 PASS。

## 4. 外部 stop 释放与按需拔插回归

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/stop-before.log" --wait 0 --seconds 5 \
  --command 'run n1_contract_probe'

"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/stop-after.log" --wait 0 --seconds 10 --command stop

"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/reopen-after-stop.log" --wait 0 --seconds 90 \
  --command 'run n1_contract_probe'
```

观察 stop 后灯灭、再运行后灯亮且两键可用。历史 `n1_lifecycle_probe` 的带电拔插
证据保留为当时的软件恢复观察。V1.2 指南要求关机或关闭对外电源后插接；当前没有
独立子板电源入口，后续采用关机、换板、POWER 开机的顺序，见
[硬件资料核对](lessons/esp-mosaico-v1.2-reference.md)。跨重启换槽不能证明同一 VM
中的旧句柄失效或存活；相关主机测试与历史热插拔实测分别记录。

## 5. 恢复默认固件与稳定性

若使用过诊断版，结束后按上一节的完整校验与自动重启流程恢复同轮默认镜像。
同轮镜像及完整指纹以[现场镜像身份](N1_HAPTIC_LIVE_20261023.md#当前运行镜像)为准，
避免把早期马达归档当作最新触摸 / 清灯修复版本；如果未换固件，可直接采样。
运行时重启可清空触摸统计，串口重连或重启 Lua 不会清空。回主页后开始采集：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/default-stability-touch.log" --wait 0 --seconds 180 \
  --command status --command ls
```

- 前两分钟静置，后一分钟对屏幕做至少 20 次独立点按，按下 / 松开清楚分离。交互板电容键
  不会增加屏幕触摸统计。记录期间画面响应和 USB 是否断开。
- 触摸 `n` 必须实际增长；既有门槛为固件侧 P95 ≤ 50 ms。该统计是触摸报告读完到首个
  QSPI 字节，排除了触摸控制器扫描 / IRQ 等待与面板出光时间，不能称为端到端触摸延迟。
- 保存稳定段 fps、`drop` 及错误计数起止值。此前主页 18–19 fps、全屏 Lua 文本 11–13 fps
  属于不同场景；新增采样是否造成回归需要相同固件配置、页面、操作和采样时长的旧版 A/B。
- `ierr` / `berr` 是累计值，比较本窗口增量；记录每条 CST9220 Header 警告。若再次增长或
  断连，保留前后完整日志继续定位，不能把“恢复了”当作稳定性通过。
- 核对 `/system`、`/data` 挂载与原有应用。最终待验收镜像以本次默认 bin 指纹为准，
  后续若修复马达，应对修复后的新镜像重做马达和受影响路径的验证。

## 6. 电气测量

由具备接线条件的人员测 GPIO8：预期 PWM 约 5 kHz；`angry` 为两段约 160 ms、间隔约
100 ms、占空比约 90%。正式 `error` / `panic` 的 100% 以两个连续高电平段
100H/100H 编码，burst 内预期保持高，结束恢复低。非零低电平不是这里的 RMT 编码要求；
实物触感不能代替电气占空比或持续时间测量。
用短预设连续请求测 pattern 启动限速，必须区分同一 pattern 内的多个 burst：**150 ms
限制的是相邻 pattern 的开始时间，不是每段 burst 的间隔**。目前四次 angry 探针的请求
间隔为 3 秒，不能单凭它验证 150 ms 下限。默认固件上配合逻辑分析仪运行已准备的有限
密集请求探针：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/haptic-rate.log" --wait 0 --seconds 20 \
  --push examples/lua-apps/n1_haptic_rate_probe
```

它等待 5 秒，发 20 次 `thinking` 请求、每次间隔标称 40 ms，最后等 1 秒。该预设为
单段 30 ms、35% duty；pending 请求可以合并，因此无需也不应期待 20 个物理 pattern。
要求仪器取得至少 3 个实际启动沿，并逐项检查间隔 ≥ 150 ms；未测到足够输出时保留
未通过 / 未覆盖，脚本的 `[N1HR] END` 只说明请求阶段结束。此新探针尚未在设备上运行。

## 7. 左槽按键与能力边界

先关机再把交互板移至左槽，用 POWER 开机，等待日志确认识别和触摸键校准完成，再运行接口探针：

```sh
"$N1_PY" scripts/n1-device-session.py --port "$N1_RUNTIME" \
  --log "$N1_LOG_DIR/left-slot-contract.log" --wait 0 --seconds 90 \
  --push examples/lua-apps/n1_contract_probe
```

应打开 GPIO13 / GPIO12，拒绝非活动的 GPIO11 / GPIO10；分别做单击、双击和 2 秒长按。
左槽灯带当前不受支持，LED 应走 `hardware unavailable` 分支，其物理灯带项为预期 SKIP。
核对其余接口无 FAIL，并记录实际手势；结束后如需恢复右槽，同样先关机再换槽。正式默认镜像的 `54/55` 日志已确认：GPIO13/12 各2次双击，分别2/1次完整长按，
实际保持2128/2080ms及1761ms；GPIO13另一次1478ms保持按阈值归为短按。操作者确认
正常；非活动GPIO11/10被拒绝、LED不可用分支通过。`56`删除临时探针。

## 结项记录

每次会话在新证据目录保存一份观察表，记录固件 / 探针指纹、时间及以下结果：

| 项目 | 记录 |
| --- | --- |
| 马达 4 次 angry | 每次触感；是否能分辨两段；对应 `hd p=` |
| 异常 / 正常 / stop 后灯 | 各自是否熄灭，残灯颜色和持续时间；重新运行是否成功 |
| 双键手势 | GPIO11 / GPIO10 的单击、双击、长按开始 / 松开日志 |
| 默认固件稳定段 | 起止 fps / drop / ierr / berr / drops / edrops；触摸 n / p50 / p95 / max |
| 启动 / USB | 烧录校验、挂载、应用保留、断连 / panic / 复位情况 |
| 物理波形与 A/B | 仪器文件 / 同场景对照日志，或明确 `not-run` 及缺失条件 |

只有相应日志、手工观察和必要仪器证据齐备，才把对应项更新为通过。正式预设触感与停止功能已通过，
触摸 P95 仍失败；物理波形、执行中取消及其余未完成项目保留为待测。
