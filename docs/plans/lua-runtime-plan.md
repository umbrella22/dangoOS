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
  - `canvas:present()` → `Request::Present`，任务把帧交给 plane，送完再 resume。
- 不能挂起的位置（脚本自己的 `coroutine` 里、`table.sort` 比较函数这类 C 调用内部）：
  `delay` 不等待直接返回；钩子不让出。如果距上次让出超过 `max_blocked_us`（2 秒；
  无时钟时为 `max_blocked_hooks` 2 万次）仍没让出过，就报错 `script ran too long without yielding` 终止脚本。
- 退出：系统手势（左边右滑）调用 `Vm::interrupt()`。下一次钩子在可让出时让出，`resume`
  返回 `Step::Interrupted`，随后整个状态 `lua_close`。脚本里的 `pcall` 拦不住这种退出；
  只有在不可让出位置才退化为抛 `interrupted` 错误。

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

开的标准库：`base`、`package`（只保留 `package.preload` 和应用目录搜索器）、`coroutine`、
`debug`、`math`、`string`、`table`、`utf8`。不开 `io`、`os`（官方开了，但应用里只用到
`os.time` ×7、`os.clock` ×5、`io.open` ×6，L1/L2 用宿主模块补这几个函数）。`dofile`/`loadfile` 移除。

官方模块按 skills-lab 26 个应用的实际调用次数排优先级：

| 模块 | 应用里用到的接口（次数） | 阶段 | 实现 |
| --- | --- | --- | --- |
| `display` | `open` 24、`color` 73、`load_font` 11；`canvas:` `text` 122、`line` 114、`fill_rect` 97、`fill_circle` 82、`fill_round_rect` 72、`info` 67、`fill_triangle` 43、`arc` 37、`begin` 30、`present` 29、`close` 27、`measure_text` 23、`touch` 22、`save` 17、`stroke_circle` 12、`stroke_rect` 10、`stroke_round_rect` 9、`restore` 4、`translate` 2、`clip` 2、`stats` 2、`blit` 1 | L1 | Rust 光栅器画进 480×480 RGB565 PSRAM 帧，`present` 经 plane 整帧送屏；内置 ASCII 字体 8–64 号；颜色 `0xAARRGGBB` / `"#rrggbb[aa]"` / 表三种写法；`<close>` 由 `__close` 元方法实现 |
| `delay` | `delay_ms` 32 | L1 | 协程让出；`delay_us` 忙等（≤ 1 s）；`periodic` |
| `system` | `millis` 74、`date` 8、`time` 1 | L1 | `millis/uptime` 用 `embassy_time`；没有 RTC/网络对时，`time/date` 返回开机后的伪时间并在文档注明 |
| `json` | `decode` 7、`encode` 6 | L1 | Rust 实现，行为对照官方 `lua_module_json.c` |
| `arg_schema` | `int` 19、`bool` 6、`parse` 3 | L1 | 官方 `lua_module_system/lib/arg_schema.lua`（Apache-2.0）原样放进 `package.preload` |
| `storage` | `join_path` 22、`read_file` 13、`get_root_dir` 12、`exists` 12、`write_file` 6、`mkdir` 6 | L2 | 应用目录 + 每应用可写区，放在 flash 数据分区 |
| `imu` | `new` 5 | L2 | 读传感器任务已有的 BMI270 数据 |
| `audio`、`lvgl` | 9 个应用 | 不做 | RFC 0006 已排除；这些应用在网格里显示为不可用 |

## 8. 阶段验收

| 阶段 | 验收 | 状态 |
| --- | --- | --- |
| L0 | 主机测试 17 项（时间切片、print 格式、标准库、模块函数、参数错误、让出、中断、`pcall` 拦不住中断、不可让出循环被终止、内存上限）；真机开机自检打印 `hi from Lua 5.5`，记录耗时、切片数、最长切片、内存 | 主机通过；真机见第 9 节 |
| L1 | 官方 `display_contract.lua` 在主机光栅器上通过；`snake`、`game_2048` 原样运行；运行时狐狸主页之外的 UI 不掉帧 | 待做 |
| L2 | `launcher.json` 解析，应用出现在网格；USB 串口推送 + 热重载 | 待做 |

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
