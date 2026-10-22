# L2 上机复盘：开机循环重启与文件系统挂不上

> 2026-10-22。对应提交 `f4064e7`（L2）和其后的清理提交。
> 代码：`crates/mosaico-s31/src/crashlog.rs`（开机崩溃记录）、
> `crates/mosaico-s31/src/drivers/storage.rs`（flash 访问与挂载）、
> `crates/mosaico-lua/build.rs`（TJpgDec 改名）、`scripts/check-rom-symbols.sh`、
> `scripts/push-app.py`。

L2 在主机上测试全绿，第一次上机却连续遇到四个问题：

1. 开机即循环重启，USB 来不及出现，没有任何日志。
2. 崩溃原因：我们链接进去的 JPEG 解码函数被 S31 ROM 里的同名函数顶替。
3. 不再崩溃以后，`/data` 和 `/system` 仍然挂不上：esp-storage 认为 flash 大小是 0。
4. 工具链和线材上的小问题：端口闪断、引导程序回滚到出厂固件、主机脚本报错。

## 1. 开机循环重启，没有日志

### 现象

- 刷入 hub16 后，点 RESET，屏幕闪一下 logo 就重启，无限循环。
- Mac 上看不到 `/dev/cu.usbmodemmosaico_s31_r11`，日志文件是 0 字节。

### 为什么看不到日志

- 日志只走 USB CDC。USB 枚举、主机打开端口、设 DTR，前后要几百毫秒到一两秒。
- 崩溃发生在这之前，esp-backtrace 打印的 panic 信息写进了一个还没人读的缓冲区，随复位一起消失。

### 做法：让崩溃“活过”复位

逐步加了四样东西，每一样都是为了下一次能看到更多：

1. **崩溃记录存进 RTC 快速内存**（`crashlog.rs`）。自己写 `#[panic_handler]`，
   把消息、文件:行号、次数、当时所处的阶段写进
   `#[esp_hal::ram(unstable(rtc_fast, persistent))]` 的结构体，然后软件复位。
   这块内存软件复位后不清零。（为此去掉了 esp-backtrace，两个 Cargo.toml 都要删。）
2. **USB 尽早起来**：USB 栈挪进一个独立的 embassy 任务，`esp_rtos::start` 之后立刻 spawn。
3. **上次崩溃过就等主机**：启动时发现上次 panic，最多等 5 s 让主机连上，再打印记录。
4. **跳过出事的阶段**：记录里有阶段（Running / Storage / AppScan），下次启动跳过它。
   这样设备至少能起来、USB 可用，下一版固件才刷得进去，日志也看得到。

另外，第一次打印的崩溃信息被单行长度截断，看不到 PC 地址，于是改成每 100 字符分段打印。

### 拿到的记录

```text
[WARN] [boot] reset Some(CoreSw) after panic #2 in Storage:
[WARN] [boot]   [ProCpu] Exception 'Load address misaligned (4)' mepc=0x401e81a6, mtval=0x00000269
[WARN] [boot] skipping Storage this boot
```

- `mepc`：出错指令的地址。`mtval`：要访问的地址（0x269，明显不是合法指针）。
- 阶段是 Storage，说明是挂载存储、扫描应用这一段。

## 2. 崩溃原因：ROM 里的同名函数

### 定位

用 Mac 上的工具链把 `mepc` 还原到源码（必须是刚刷进去的同一个 ELF）：

```sh
T=~/esp-tools/riscv32-esp-elf/bin
E=target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw
$T/riscv32-esp-elf-addr2line -f -i -C -e $E 0x401e81a6
$T/riscv32-esp-elf-objdump -d --start-address=0x401e8160 --stop-address=0x401e81c0 $E
```

结果在 `mosaico_jpeg.c` 的 `in_func`：TJpgDec 的读数据回调，从 `JDEC` 结构里取我们的上下文指针。
回调本身很简单，不应该出错。问题在于是谁调用了它。

### 原因

- S31 的 ROM 里自带一份 TJpgDec。`esp-rom-sys/ld/esp32s31/rom/esp32s31.rom.ld` 里写着
  `jd_prepare = 0x2f800110;` 和 `jd_decomp = 0x2f800114;`。
- 这是普通赋值，不是 `PROVIDE(...)`。链接脚本里的普通赋值会覆盖目标文件里的同名定义，
  所以我们 vendored 的 `jd_prepare` 被静默替换成 ROM 地址，链接器不报错。
  （S3 的脚本用的是 `PROVIDE`，只在没人定义时才生效，所以 S3 上不会出这个问题。）
- ROM 版的 `JDEC` 结构布局和我们这份 r0.03 不一样，它回调 `in_func` 时传进来的结构字段是错位的，
  读出一个垃圾指针，于是 `Load address misaligned`。

### 修复

`mosaico-lua/build.rs` 编译 TJpgDec 时加
`.define("jd_prepare", "mosaico_jd_prepare").define("jd_decomp", "mosaico_jd_decomp")`，
让我们的函数换个名字，不再和 ROM 冲突。

### 防止再犯

新增 `scripts/check-rom-symbols.sh`：从 S31 的 ROM 链接脚本里取出所有被赋值的符号名，
和我们 C 静态库里导出的全局符号求交集，有交集就报错。现在 623 个 C 符号，无冲突。
**以后每 vendor 一份 C 库（尤其是 zlib、TJpgDec、miniz、cJSON、mbedtls 这类 ROM 里常带的），
`cargo fw` 之后都跑一次。**

## 3. 文件系统挂不上：flash 大小被认成 0

### 现象

修好崩溃后不再重启，但推送应用失败：`/data/apps/.incoming/hello_push: not found`。启动日志：

```text
fs: partition table unreadable: StorageError
fs: no system partition
fs: no room for /data in main_app
```

### 定位

- `StorageError` 来自 esp-bootloader-esp-idf，它把 esp-storage 原本的错误吞掉了，看不出具体原因。
- 读 esp-storage 源码：容量来自 `get_flash_size()`，通过 SPI1 读 flash 的 RDID 再查表。
  不认识的 ID 就返回 0，之后每次读都因越界检查失败。
- 加了三行诊断日志，各打印前几个字节：esp-storage 认为的容量、从 0x0 和 0x2000 直接读到的内容、
  分区表开头：

```text
fs: flash @0x0: [ff, ff, ff, ff, ff, ff, ff, ff]
fs: flash @0x2000: [e9, 03, 02, 4f, b0, a2, 06, 2f]
fs: partition table: [aa, 50, 01, 00, 00, 90, 00, 00, ...]
fs: flash size: header 16384 KiB, partitions 16384 KiB, esp-storage 0 KiB
```

两个发现：

1. esp-storage 的容量确实是 0：S31 的 RDID 不在它的表里。
2. **S31 的二级引导不在 0，而在 0x2000**（0 处全是 FF），和 C5/P4 一样。第一版只看 0x0 的镜像头，
   也拿不到大小。镜像头第 3 字节高 4 位是 flash 大小代码：`0x4f` 的 4 表示 16 MB。

### 修复

- `drivers/storage.rs` 不再用 `FlashStorage`，改经 `esp_storage::ll`
  （`spiflash_read` / `spiflash_write` / `spiflash_erase_sector` / `spiflash_unlock`）直接调 ROM。
  `ll` 的每个调用都包在临界区里，与 `FlashStorage` 内部的做法相同。
- 越界检查自己做。flash 大小先看 0x0 和 0x2000 处的镜像头（`0xE9` 魔数），
  取不到就用分区表里最后一个分区的末端，向上取到 2 的幂。
- 分区表自己解析：每项 32 字节，`AA 50` 开头，然后是类型、子类型、偏移、大小、16 字节标签。
  esp-bootloader-esp-idf 的 `PartitionTable::new` 是私有的，只能连同它那条读 flash 的路径一起用，
  所以没有用它。
- 修好后诊断日志删掉，只留一行 `fs: flash 16 MiB`。

结果：`/system` 只读挂上，读到 dino 和 flappybird；`/data` 第一次启动时格式化，推送、热重载、
重启后保留都正常。

## 4. 其他小坑

### 屏幕上出现了“很高级的系统”

连续崩溃一段时间后，引导程序回滚到出厂的 `vibe_mode` 分区（0x20000，乐鑫官方固件）。
这说明我们的 `main_app` 起不来，不说明板子坏了。重新刷 main_app 就好，0x20000 一直没碰过。

### 下载口闪断

- 刷机时报 `could not open port ... Device not configured`，之后 Mac 上完全看不到板子。
- 原因是 USB 线没插牢，重插后正常。
- 刷机命令改成先轮询等 `/dev/cu.usbmodem113401` 出现，再等 1–3 s 才开始刷，
  免得端口刚出现、还没枚举完就去打开。

### `reboot` 命令报 Traceback

板子复位时 USB 断开，经常赶在回复之前，主机脚本读串口时抛异常。现在 `push-app.py reboot`
把断开当成成功。

### zsh 里 `echo ==1` 会出错

zsh 把 `=word` 展开成命令路径，`echo ==1` 报 `=1 not found`，后面的命令都没跑。
在脚本里作分隔标记时，用 `echo STEP1` 这类普通单词。

### 日志在没人读时会丢

console 等 DTR 才发送，主机没连上的那段时间里，队列满了就丢（`ldrop=108`）。
要看应用启动那一刻的日志，用 `push-app.py push <目录> --run --follow`：
同一个连接先推送、再跟日志，不会错过。

## 5. 可以带走的经验

### 崩溃信息要能活过复位

- 只有 USB 日志的板子，开机早期的崩溃看不见。panic 信息写进 RTC 内存，下次启动再打印，
  是性价比最高的做法。
- 记下“当时在做什么”（阶段），下次启动跳过，设备才不会卡死在重启循环里。

### 地址先用 addr2line 还原

- `mepc` 用 addr2line 加反汇编看。要用刚刷进去的那个 ELF，重新编译后地址就对不上了。
- 一个很简单的函数在不该出错的地方出错时，要怀疑调用它的人是不是你以为的那个。

### 链接脚本可以覆盖你的函数

- ESP 的 ROM 链接脚本会给大量常见 C 库函数（TJpgDec、zlib、miniz、部分 libc）赋地址。
  S31 的脚本对其中一部分（比如 `jd_prepare`）用普通赋值而不是 PROVIDE，会无声地覆盖同名定义。
- vendor C 代码后跑 `scripts/check-rom-symbols.sh`。有冲突就在 `build.rs` 里用 `-D` 改名。

### 新芯片别信 HAL 的“自动探测”

- 尺寸、地址这类值，要看 HAL 实际返回了什么（这次是 0），再看 flash 里实际是什么（引导在 0x2000）。
- 包装层把错误吞掉时，绕过它直接调底层（`ll`），把原始返回值和原始字节打印出来。

### 诊断日志：打印原始数据，修好就删

- 先打印前几个字节和原始返回码，不要先写解释。数据会直接告诉你哪个假设错了。
- 修好之后删掉，或者合并成一行有用的信息。

## 6. 下次遇到类似问题的检查清单

**开机循环重启：**

1. 看 `[boot] reset ... after panic` 的记录：消息、`mepc`、`mtval`、阶段。
2. 用同一个 ELF 对 `mepc` 跑 addr2line 和 objdump。
3. 出错点在 vendored C 代码里 → 跑 `scripts/check-rom-symbols.sh`。
4. 一直没有记录 → 确认 USB 任务是否最先 spawn、崩溃是否早于 `crashlog` 初始化。
5. 屏幕显示出厂固件 → 引导程序回滚了，重新刷 main_app 即可。

**存储或外设读不出来：**

1. 打印 HAL 给出的参数（容量、ID、频率）和原始返回码。
2. 直接读几个已知地址（0x0、0x2000、0x8000）的原始字节，对照魔数：
   镜像头 `E9`，分区表项 `AA 50`。
3. 确认这颗芯片的布局（引导偏移、flash 大小），不要沿用别的芯片的默认值。
4. 包装层吞错误 → 换底层接口，保留同样的临界区和缓存处理。

**刷机：**

- 只写 main_app `0x210000`。先轮询等下载口出现再刷；端口时有时无就先换线、重插。
