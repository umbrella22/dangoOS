# Lua 5.5 运行时设计（RFC 0006 第 5 节的落地方案）

> 2026-10-22。范围：`crates/mosaico-lua` 与固件 `tasks/lua.rs`。阶段表见
> [RFC 0006 第 5 节](hub-shell-plan.md#5-lua-应用层l-阶段)。

## 1. 基线

| 项 | 版本 | 用途 |
| --- | --- | --- |
| Lua | 5.5.1，`lua-5.5.1.tar.gz` sha256 `1c4b4068d67061f2a2231ad2b5422e77acea1487ea9890f6320af614f4373dce`（与 lua.org/ftp 一致） | 解释器，原样放在 `crates/mosaico-lua/vendor/lua-5.5.1`（MIT） |
| esp-claw | `345073de68179542c307e303df81e38baadf77f0`（2026-10-19） | 模块接口基线：`components/lua_modules/lua_module_*`、`cap_lua_runtime.c`；`idf_component.yml` 要求 `lua ^5.5.1` |
| esp-claw-skills-lab | `57969b1e5891b5ad68e3d2433f1e9dde3ab1fcc0`（2026-10-21） | 兼容性样本：`apps/*`（26 个应用） |

升级任一基线都要重跑第 7 节的契约测试。

## 2. 结构

```
crates/mosaico-lua/            no_std + alloc，主机可测
  vendor/lua-5.5.1/            PUC-Rio 源码，未改动（去掉 lua.c luac.c liolib.c loslib.c linit.c）
  csrc/mosaico_lua_user.h      -include 进每个 Lua 源文件：小数点固定 '.'、strcoll→strcmp、日志出口
  csrc/mosaico_lua_glue.c      开库、print、warn、钩子、Rust 函数跳板、traceback
  build.rs                     cc 编译；固件目标再链接工具链 newlib 的 libc/libm/libgcc
  src/vm.rs                    Vm：一个 lua_State + 一个脚本协程
  src/call.rs                  Call/Function：用 Rust 写的模块函数
crates/mosaico-s31/
  src/drivers/libc.rs          newlib 需要的 _malloc_r 系列、系统调用桩、__getreent
  src/tasks/lua.rs             Lua 任务（L0：开机自检）
```

不用 mlua：它依赖 std。绑定是手写的几十个函数，够用且可审。

## 3. 错误处理规则（最重要）

Lua 用 `longjmp` 抛错。`longjmp` 越过 Rust 栈帧是未定义行为，所以：

1. Rust 不调用任何 `luaL_check*`、`lua_error`、`luaL_error`。
2. Rust 写的模块函数签名是 `fn(&mut Call) -> Result<Ret, Error>`。C 跳板 `trampoline` 调它，拿到
   `Err` 后在 C 栈帧里调 `lua_error` / `luaL_argerror`。报错格式与官方一致：
   `app.lua:3: bad argument #1 to 'fill_rect' (number expected, got nil)`。
3. 会分配内存的 API（push 字符串、建表）理论上也会抛内存错误。Rust 函数运行期间不执行内存上限，
   只有 PSRAM 真用完才会失败，那时直接中止，与 Rust 自身分配失败相同。
4. 建状态、注册模块、取 traceback 都在 C 里用 `lua_pcall` 包住。
5. Rust 函数里不在 Lua API 调用前后持有需要 Drop 的值（读参数只借用栈上的字符串）。

## 4. 调度

一个固件只跑一个 Lua 应用，放在单独的 Embassy 任务里，和 UI 任务在同一个线程执行器上协作调度。

- 脚本跑在独立协程上（`lua_newthread` + `lua_resume`）。
- 切片按时间算：计数钩子每 `hook_interval`（200）条指令进一次 Rust，读 `Limits::clock`
  （固件里是 `esp_hal::time::Instant`，微秒），本片跑满 `slice_us`（4 ms）就 `lua_yield`。
  `Vm::resume` 返回 `Step::Slice`，任务 `yield_now()` 让 UI 和输入先跑。
  按指令数切片不可靠：一条 `string.format` 可能在软浮点 `snprintf` 里花上百微秒，
  实测按 8000 条指令切片时最长一片 133 ms。没有时钟时（主机测试）仍按 `slice_hooks`（8）次计数。
- 占空上限：Lua 连续运行累计 16 ms 而没有 `Sleep` 请求时，任务改为 `Timer::after_millis(1)`，
  让定时器驱动的传感器任务能按时醒来；否则 `yield_now()`。
- 模块函数可以返回 `Ret::Yield(Request)` 挂起脚本：
  - `delay.delay_ms(n)` → `Request::Sleep{ms}`，任务 `Timer::after` 后再 resume。
  - `canvas:present()` → 官方 C 代码算完脏矩形后经 `lua_yieldk` 发 `Request::Present`；
    Lua 任务把矩形（指针 + 行距）交给 UI 任务，UI 在 plane 空闲时拷进 LVGL 的 shadow、
    按合并后的脏区上传，然后通知 Lua 任务 resume。页面滑动期间也等着不丢帧
    （脚本只重画变化的部分，丢一帧会留下启动器残影）。
- 不能挂起的位置（脚本自己的 `coroutine` 里、`table.sort` 比较函数这类 C 调用内部）：
  `delay` 按官方 `vTaskDelay` 的语义忙等到时间过去，`present` 不等 UI 直接返回；钩子不让出。如果距上次让出超过 `max_blocked_us`（2 秒；
  无时钟时为 `max_blocked_hooks` 2 万次）仍没让出过，就报错 `script ran too long without yielding` 终止脚本。
- 退出：系统手势（左边右滑）调用 `Vm::interrupt()`。下一次钩子在可让出时让出，`resume`
  返回 `Step::Interrupted`，随后整个状态 `lua_close`。脚本里的 `pcall` 拦不住这种退出；
  只有在不可让出位置才退化为抛 `interrupted` 错误。官方在 `delay` 里抛可捕获的
  `stop requested`，这里不抛，统一由钩子中断，脚本的 `pcall` 不会吞掉退出。
- 关状态前按注册的逆序调用各模块的清理函数（`cap_lua_register_cleanup`，放在 `pcall` 里），
  display 借此释放帧缓冲并通知 UI 恢复 LVGL。

官方 `cap_lua_runtime.c` 用 FreeRTOS 任务 + `vTaskDelay` 阻塞；这里改成协程让出，
脚本看到的行为相同（`delay_ms` 返回时时间已经过去）。

## 5. 内存

- `lua_Alloc` 走 `RawAllocator`：固件里是 `esp_alloc::HEAP.alloc_caps(External)`，即 PSRAM。
- 每个应用上限 4 MiB（`tasks::lua::LIMITS`），超限返回 NULL，Lua 先做一次紧急 GC 再报
  `not enough memory`。`Vm::memory_used/peak` 给出用量。
- 缩小内存块永不失败（分配新块失败时保留旧块），满足 Lua 对分配器的要求。
- newlib 自己偶尔要 `malloc`（如 `strtod` 处理长数字），走 esp-alloc 的 C 兼容 `malloc`（内部 RAM）。
- 应用退出即 `lua_close`，所有内存一次归还。
- 堆算法：`.cargo/config.toml` 设 `ESP_ALLOC_CONFIG_HEAP_ALGORITHM = "TLSF"`。默认的 LLFF
  在 Lua 大量小块分配时线性查找，自检最长切片 277 ms；换 TLSF 后分配为常数时间。
- 性能：Lua 栈和对象都在 PSRAM，解释器约 130 万条指令/秒。以后可以把 Lua 栈和小对象
  放到内部 RAM（按块大小分流），这是 L1 之后的优化项。

## 6. C 库层

固件链接 riscv32-esp-elf 工具链自带的 newlib（multilib `rv32imafc_zicsr_zaamo_zalrsc/ilp32f`），
拿到 `snprintf`（`%.14g` 浮点格式与官方一致）、`strtod`、`setjmp/longjmp`（保存 fs0–fs11）、`libm`。
`drivers/libc.rs` 补上 newlib 期待的操作系统接口：

| 符号 | 处理 |
| --- | --- |
| `_malloc_r` `_free_r` `_calloc_r` `_realloc_r` | 转给 esp-alloc 的 `malloc` 等（esp-alloc `compat` 特性已提供） |
| `_open_r` `_read_r` `_write_r` `_close_r` `_lseek_r` `_fstat_r` `_isatty_r` `_fcntl` | 返回 -1 / `ENOSYS`：没有文件系统，`fopen` 得到 NULL |
| `_sbrk_r` `_kill_r` `_getpid_r` | 桩 |
| `__getreent` | 返回 newlib 的全局 `_impure_ptr`（只有 Lua 任务调用 newlib） |
| `__stack_chk_guard` | 定义在固件里，避免拉进 newlib 自带的第二个 `__stack_chk_fail` |
| `abort` `_exit` | Rust panic（只有 Lua 内部 panic 才会到这里） |

`lua_Integer` 保持 64 位、`lua_Number` 保持 double（软浮点），与官方 `espressif/lua` 默认一致，
`math.maxinteger`、整数除法、`string.format` 的结果都和官方相同。

## 7. 标准库与模块对照

开的标准库：`base`、`package`（只保留 `package.preload` 和 Lua 文件搜索器，C 模块搜索器去掉）、
`coroutine`、`debug`、`math`、`string`、`table`、`utf8`、`os`、`io`（L2）。`dofile`/`loadfile`
L2 起保留（只读文本块，`mode "t"`）。

- `os`：官方 `loslib.c` 原样编译（`csrc/mosaico_loslib.c` 用宏把 `time`/`clock` 接到宿主时钟，
  `system`/`tmpnam` 返回失败；L2 起 `remove`/`rename` 走文件系统），`os.exit` 置 nil。
- `debug`：只留 `traceback`、`getinfo`，与官方 `cap_lua_restrict_debug` 相同。
- `io`：L2 起开，官方 `liolib.c` 原样编译，经 newlib 系统调用落到 `mosaico-fs`（第 11 节）。
- 全局 `args`：启动参数 JSON 经 `json.decode` 得到的表，没有参数时为空表。

L1 的官方模块都是 vendored C 源码原样编译（来源见 `crates/mosaico-lua/vendor/NOTICE.md`），
ESP-IDF 头文件在 `csrc/esp_shim/` 里打桩，`csrc/mosaico_platform.c` 把 `display_service`、
堆、计时等接到 Rust 宿主（`mosaico_host_*`）。`delay`、`system` 两个模块官方实现依赖 FreeRTOS，
在 `mosaico_platform.c` 里按官方接口重写。

官方模块按 skills-lab 26 个应用的实际调用次数排优先级：

| 模块 | 应用里用到的接口（次数） | 阶段 | 实现 |
| --- | --- | --- | --- |
| `display` | `open` 24、`color` 73、`load_font` 11；`canvas:` `text` 122、`line` 114、`fill_rect` 97、`fill_circle` 82、`fill_round_rect` 72、`info` 67、`fill_triangle` 43、`arc` 37、`begin` 30、`present` 29、`close` 27、`measure_text` 23、`touch` 22、`save` 17、`stroke_circle` 12、`stroke_rect` 10、`stroke_round_rect` 9、`restore` 4、`translate` 2、`clip` 2、`stats` 2、`blit` 1 | L1 | 官方 `lua_module_display`（C）原样编译，帧缓冲在 PSRAM；内置 12–48 px 十种字号全编进固件，按 `font_size` 取同高字体（官方只编一种再缩放，非原尺寸发虚；字宽都是 h/2，排版不变）；`present` 见第 4 节；图片解码（`lua_image`）打桩返回不支持 |
| `delay` | `delay_ms` 32 | L1 | `delay_ms` 协程让出；`delay_us` 忙等；`periodic` 按 `xTaskDelayUntil` 语义 |
| `system` | `millis` 74、`date` 8、`time` 1 | L1 | `millis/uptime` 用开机时钟；没有 RTC/网络对时，`time/date` 从开机时刻按 1970-01-01 起算；`ip` 返回 nil；`heap.get_info` 读 esp-alloc 统计 |
| `json` | `decode` 7、`encode` 6 | L1 | 官方 `lua_module_json.c` + cJSON 1.7.18 |
| `arg_schema` | `int` 19、`bool` 6、`parse` 3 | L1 | 官方 `lua_module_system/arg_schema.lua`（Apache-2.0）原样放进 `package.preload` |
| `storage` | `join_path` 22、`read_file` 13、`get_root_dir` 12、`exists` 12、`write_file` 6、`mkdir` 6 | L2 | 官方 `lua_module_storage.c` 原样编译，`get_root_dir` 为 `/data` |
| `imu` | `new` 5 | L2 | 读传感器任务已有的 BMI270 数据 |
| `audio`、`lvgl` | 9 个应用 | 不做 | RFC 0006 已排除；这些应用在网格里显示为不可用 |

## 8. 阶段验收

| 阶段 | 验收 | 状态 |
| --- | --- | --- |
| L0 | 主机测试 17 项（时间切片、print 格式、标准库、模块函数、参数错误、让出、中断、`pcall` 拦不住中断、不可让出循环被终止、内存上限）；真机开机自检打印 `hi from Lua 5.5`，记录耗时、切片数、最长切片、内存 | 主机通过；真机见第 9 节 |
| L1 | 官方 `display_contract.lua` 在主机光栅器上通过；`snake`、`game_2048` 原样运行；运行时狐狸主页之外的 UI 不掉帧 | 主机通过（`mosaico-lua` 24 项：contract 全过 delta=0、像素、释放、delay、system/os/json/arg_schema、snake、2048 各自跑到结束）；真机通过，见第 10 节 |
| L2 | `launcher.json` 解析，应用出现在网格；USB 串口推送 + 热重载；`io`/`storage` | 主机通过（`mosaico-fs` 10 项含官方镜像只读挂载；`mosaico-lua` 28 项含 io/storage、官方 Dino/Flappy Bird、推送示例；`mosaico-s31` 199 项含注册表、推送协议）；真机见第 11 节 |

### 启动器与 Lua 页面

启动器网格每行 3 个，先是 Settings / Sensors / Timer，后面是注册表里的 Lua 应用
（`hub::AppId::Lua(i)`，见第 11 节）。超过两行时网格可以上下滚动。

1. 点开 Lua 应用 → 页面滑入（LVGL 先画“正在启动…”），同时 `lua::start` 启动脚本。
2. 脚本 `display.open` 后 UI 不再跑 LVGL，改为拷贝脚本提交的帧；非系统手势的触摸送给
   脚本（`canvas:touch`）。
3. 系统手势照常：左边右滑回启动器，底边上滑回主页。Lua 页面画满全屏，底边手势区从
   下方 60 px 收窄到 28 px（`FULL_SCREEN_BOTTOM_BAND_Y`），否则 2048 从底部往上滑会被当成回主页。
   离开时 `lua::stop` 中断脚本，状态关闭、帧缓冲释放。
4. 脚本自己结束或报错 → 自动回启动器，错误写日志。
5. 灯效、插拔模块的反应照常，与其他应用页相同。

## 9. L0 实测

固件增量（`riscv32-esp-elf-size`，对比 fb50cee）：text +229,172 B（2,114,084 → 2,343,256），
data +284 B，bss +472 B。

真机开机自检（S3 + 32 MiB PSRAM，同一脚本三次刷机）：

| 版本 | fib(20) | format ×2000 | sort | loop 10 万 | alloc+gc | 总耗时 | 切片数 | 最长切片 | 传感器最大延迟 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LLFF，8000 指令/片 | — | — | — | — | — | 811 ms | — | 277 ms | — |
| TLSF，8000 指令/片 | 130 ms | 435 ms | 33 ms | 457 ms | 123 ms | 1183 ms | 73 | 133 ms | 3.02 s |
| TLSF，4 ms 时间片 + 占空上限 | 141 ms | 802 ms | 42 ms | 594 ms | 170 ms | 1754 ms | 201 | 43.8 ms | 236 ms |

- 内存：状态 13,319 B，峰值 558,513 B，结束时 105,269 B；关闭后堆空闲比运行前少 616 B。
- 第三版总耗时变长是有意的：每 16 ms 让出 1 ms，其余时间给 UI 和传感器；UI 保持 18–19 fps。
- 剩下的 43.8 ms 是单个 C 调用（`collectgarbage("collect")` 全量回收 50 万字节级的 PSRAM 堆），
  钩子进不去。应用里不会频繁手动全量 GC；增量 GC 的单步由 Lua 自己控制，不受影响。

## 10. L1 实测

固件 text 2,497,894 B（L0 为 2,343,256 B，+154,638 B，其中十种字号约 66 KB）。

真机（S3 + 32 MiB PSRAM），启动器里反复进出 Snake / 2048，左边右滑和底边上滑退出：

| 应用 | 运行 | 帧数 | 切片数 | 最长切片 | 内存峰值 |
| --- | --- | --- | --- | --- | --- |
| game_2048 | 42.2 s | 41 | 2341 | 14.7 ms | 155,518 B |
| game_2048 | 16.3 s | 17 | 893 | 12.6 ms | 151,022 B |
| snake | 9.9 s | 57 | 230 | 10.8 ms | 116,724 B |
| snake | 3.5 s | 20 | 84 | 10.1 ms | 93,830 B |

- 所有切片中最长 20.8 ms（2048 启动时）。plane 统计在 18–19 fps，`drop=0`。
- 传感器最大延迟 127 ms（L0 为 236 ms）。
- 退出后堆空闲回到进入前的水平（多次进出都是 15,578,4xx B），没有泄漏。
- 第一次上机发现两处问题，已修：2048 从底部往上滑被当成回主页（底边手势区收窄，见第 8 节），
  以及非 24 px 的字发虚（内置字号全编进去，见第 7 节）。

## 11. L2：文件系统、注册表、推送

### 存储布局

| 挂载点 | 位置 | 内容 |
| --- | --- | --- |
| `/system` | `system` 分区（0xD00000，LittleFS，750 × 4 KiB），只读 | 出厂镜像：`apps/`（官方应用）、`scripts/builtin/`（共享 Lua 库） |
| `/data` | `main_app` 分区最后 2 MiB（0x7E0000..0x9E0000），可写，首次启动格式化 | `apps/`（推送的应用）、应用自己的数据 |

- `crates/mosaico-fs`：littlefs 2.11.3（BSD-3）+ 挂载表 VFS + newlib 系统调用
  （`_open_r`/`_read_r`/`_write_r`/`_lseek_r`/`_close_r`/`_fstat_r`/`_stat_r`/`_unlink_r`/
  `_rename_r`/`mkdir`/`opendir` 等）。所有调用都在同一个执行器线程上同步完成，不需要锁等待。
- newlib 的链接顺序由 `mosaico-fs/build.rs` 负责，保证我们的系统调用先于 newlib 自带桩。
- 固件镜像不能超过 0x5D0000 B（0x7E0000 − 0x210000），`scripts/flash-app.sh` 写入前检查。

### 注册表

与官方 `app_registry` 规则一致：按 `/data/apps`、`/system/apps`、内置（Snake、2048）的顺序扫描，
同 id 先到先得（推送的同名应用遮住出厂的）；`launcher.json` ≤ 4 KiB，`schema_version` 1，
id 为 1–63 个 `[A-Za-z0-9_-]`，`entry` 为相对路径；最多 64 个应用。

- 入口脚本里 `require` 的模块不在本固件里（`lvgl`、`audio`、`image`、`thread` 等）时，
  格子变暗，点开显示“不支持：缺少 X”。可运行的排在前面，再按 `order`、标题。
- 图标：`assets/icon.jpg` 用 TJpgDec 解码、最近邻缩到 96 px 以内，转 RGB565 缓存（最多 48 个）。
- `package.path` 为 `<应用 scripts 目录>/?.lua;…/?/init.lua;/system/scripts/builtin/?.lua;…lib/?.lua`。

### USB 推送协议

日志口（CDC-ACM）同时收命令：以 `@mosaico ` 开头的行是命令，其余忽略；每条命令一行回复
`@ok …` / `@err …`，和日志行混在一起，主机按前缀挑出来。

```
@mosaico hello                      -> @ok hello mosaico-push 1
@mosaico begin <id>                 -> @ok begin <id>        （清空 /data/apps/.incoming）
@mosaico file <rel> <size> <crc32>  -> @ok ready <rel>
  <size> 字节原始数据                 -> @ok file <rel>        （CRC-32 为 zlib 的，十六进制）
@mosaico commit <id> [run]          -> @ok commit <id>       （校验清单和入口，替换 /data/apps/<id>）
@mosaico abort | stop | reload | ls | run <id> | rm <id> | status | reboot
```

- 文件路径只允许 `[A-Za-z0-9._-]` 组成、不以点开头的段，最多 6 层；单个文件 ≤ 1 MiB。
- 主机等 `ready` 再发数据，出错时数据不会被当成命令；写 flash 时 USB 自动 NAK，即流控。
- `commit` 之后 UI 任务重扫注册表：当前正在屏幕上的应用如果还在，就用新文件重启（热重载）；
  已被删除或变得不能运行就停掉回启动器。`commit … run` 直接打开该应用。
- 主机工具：`scripts/push-app.py push <目录> [--run] [--follow]`，以及 `run`/`stop`/`reload`/`status`/`reboot`/
  `ls`/`rm`。示例应用 `examples/lua-apps/hello_push`（本地模块、`io` 计数、`storage`）。

### 实测

固件镜像 2,606,608 B（上限 6,094,848 B 的 43%）。真机启动日志：

```
fs: flash @0x2000: [e9, 03, 02, 4f, ...]          （S31 的二级引导在 0x2000，0 处全是 FF）
fs: flash size: header 16384 KiB, partitions 16384 KiB, esp-storage 0 KiB
fs: /system ro @0xd00000 3000 KiB                   738/750 块已用
fs: /data rw @0x7e0000 2048 KiB (formatted)
apps: 6 in 161 ms: dino flappybird game_2048 snake app_store* album_app*
```

- `push hello_push --run`：3 个文件 1,558 B，1.4 s；再推一次，屏幕上的应用用新文件重启（热重载），
  `io` 计数器在 `/data` 里递增（第 5 次启动显示 `launch 5`）。`reboot` 之后 `ls` 仍有 hello_push。
- `app_store`、`album_app` 需要 `lvgl` 等模块，格子变暗，符合预期。

上机时踩到的两个坑，都已修：

1. S31 的 ROM 链接脚本把 `jd_prepare`/`jd_decomp` 定义成 ROM 里的 TJpgDec（JDEC 结构不同）。
   我们自带的 TJpgDec 被链接到 ROM 版本上，解第一个图标就 `Load address misaligned`，开机循环重启。
   `mosaico-lua/build.rs` 把这两个函数改名为 `mosaico_jd_*`；其余 vendored C 符号已核对，没有别的冲突。
2. esp-storage 在 S31 上按 RDID 推算的 flash 大小是 0，所有带越界检查的读都失败，分区表读不出来。
   现在 `drivers/storage.rs` 经 `esp_storage::ll` 直接调用 ROM 读写擦（每次在临界区里），
   flash 大小取 0x0 / 0x2000 处引导镜像头，取不到时用分区表末端向上取 2 的幂。

开机崩溃会记进 RTC 快速内存（`crashlog.rs`），下次启动在 USB 连上后打印，
并跳过上次崩溃时所在的阶段（挂载存储、扫描应用），避免卡死在重启循环里。
