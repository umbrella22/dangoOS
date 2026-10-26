# littlefs

面向微控制器、能够应对故障的小型文件系统。

> 本地中文译文，基于上游 v2.11.3 README。此目录只收录核心源码，文中上游测试命令需要在完整上游仓库执行；本项目文件系统测试为 `cargo test -p fs --offline`。许可证原文保留。

```text
   | | |     .---._____
  .-----.   |          |
--|o    |---| littlefs |
--|     |---|          |
  '-----'   '----------'
   | | |
```

**抗断电能力**：littlefs 针对随机断电设计。文件操作使用强写时复制保证，断电后回退到最后一个已知正常状态。

**动态磨损均衡**：面向 Flash，在动态分配的块间均衡磨损，并能检测和绕过坏块。

**有界 RAM/ROM**：适合小内存环境。RAM 使用量严格有界，不随文件系统增长而增长；没有无界递归，动态分配仅限可配置缓冲，也可提供静态缓冲。

## 示例

以下示例每次 main 执行时更新 `boot_count` 文件。写时复制使程序中断后仍能恢复到已提交的计数状态，避免文件系统损坏。示例将挂载失败视作首次启动并格式化；实际设备的挂载失败处理由应用决定，本项目实现位于 `crates/fs`。

```c
#include "lfs.h"

// 文件系统使用的变量
lfs_t lfs;
lfs_file_t file;

// 该结构体提供文件系统配置
const struct lfs_config cfg = {
    // 块设备操作
    .read  = user_provided_block_device_read,
    .prog  = user_provided_block_device_prog,
    .erase = user_provided_block_device_erase,
    .sync  = user_provided_block_device_sync,

    // 块设备配置
    .read_size = 16,
    .prog_size = 16,
    .block_size = 4096,
    .block_count = 128,
    .cache_size = 16,
    .lookahead_size = 16,
    .block_cycles = 500,
};

// 程序入口
int main(void) {
    // 挂载文件系统
    int err = lfs_mount(&lfs, &cfg);

    // reformat if we can't 挂载文件系统
    // 本示例假设此情况仅发生于首次启动
    if (err) {
        lfs_format(&lfs, &cfg);
        lfs_mount(&lfs, &cfg);
    }

    // 读取当前计数
    uint32_t boot_count = 0;
    lfs_file_open(&lfs, &file, "boot_count", LFS_O_RDWR | LFS_O_CREAT);
    lfs_file_read(&lfs, &file, &boot_count, sizeof(boot_count));

    // 更新启动计数
    boot_count += 1;
    lfs_file_rewind(&lfs, &file);
    lfs_file_write(&lfs, &file, &boot_count, sizeof(boot_count));

    // 文件成功关闭后才会更新存储
    lfs_file_close(&lfs, &file);

    // 释放使用中的资源
    lfs_unmount(&lfs);

    // 打印启动计数
    printf("boot_count: %d\n", boot_count);
}
```

## 使用方式

详细 API 说明见 [lfs.h](lfs.h) 注释。配置结构体定义块设备操作和尺寸、内存与性能的可调参数，以及避免动态分配时可选的静态缓冲。

文件系统状态由调用方分配的 `lfs_t` 保存，因此可以同时使用多个文件系统。通过 `lfs_t` 和配置即可格式化块设备或挂载文件系统。挂载后提供完整的类 POSIX 文件/目录函数，区别是文件系统结构体的分配由调用方负责。

remove、rename 等 POSIX 操作即使遇到断电也具有原子性。文件更新直到调用 sync 或 close 才真正提交到文件系统。

## 其他注意事项

littlefs 使用 C 编写，可由符合 C99 的编译器编译。所有调用都可能返回负错误码，来源包括 [lfs.h](lfs.h) 的 `enum lfs_error` 或调用方块设备操作。

配置中的 `prog` 和 `erase` 若能检测坏块，可返回 `LFS_ERR_CORRUPT`。磨损均衡不依赖这些函数的返回码，而是通过读回全部数据检查完整性。

存储若缓存写入，`sync` 必须将全部数据刷新到实际存储，并保证下一次读取来自实际存储，否则无法保证完整性。若写入不缓存、每次 read/write 都直达存储，`sync` 可直接返回 0。

## 设计

littlefs 是基于块的文件系统，用小日志存储元数据，用较大的写时复制（COW）结构存储文件数据。小日志称为元数据对，允许快速更新存储中任意位置的元数据；COW 结构紧凑保存文件数据，不产生额外的磨损放大。

两类结构均由块构成，共用块分配器。通过限制一个块每次分配允许的擦除次数，在整个文件系统内提供动态磨损均衡。

下图保留上游结构标记：root 为根，new 为新块，write 为写入。

```text
                    root
                   .--------.--------.
                   | A'| B'|         |
                   |   |   |->       |
                   |   |   |         |
                   '--------'--------'
                .----'   '--------------.
       A       v                 B       v
      .--------.--------.       .--------.--------.
      | C'| D'|         |       | E'|new|         |
      |   |   |->       |       |   | E'|->       |
      |   |   |         |       |   |   |         |
      '--------'--------'       '--------'--------'
      .-'   '--.                  |   '------------------.
     v          v              .-'                        v
.--------.  .--------.        v                       .--------.
|   C    |  |   D    |   .--------.       write       | new E  |
|        |  |        |   |   E    |        ==>        |        |
|        |  |        |   |        |                   |        |
'--------'  '--------'   |        |                   '--------'
                         '--------'                   .-'    |
                         .-'    '-.    .-------------|------'
                        v          v  v              v
                   .--------.  .--------.       .--------.
                   |   F    |  |   G    |       | new F  |
                   |        |  |        |       |        |
                   |        |  |        |       |        |
                   '--------'  '--------'       '--------'
```

详细原理和磁盘格式见固定版本的上游资料：

- [设计原理 DESIGN.md](https://github.com/littlefs-project/littlefs/blob/v2.11.3/DESIGN.md)：数据结构及设计取舍。
- [磁盘格式 SPEC.md](https://github.com/littlefs-project/littlefs/blob/v2.11.3/SPEC.md)：存储格式细节，可用于开发工具。

## 上游测试

完整上游仓库包含 PC 测试套件，使用 `bd` 目录中的[模拟块设备](https://github.com/littlefs-project/littlefs/blob/v2.11.3/bd/lfs_emubd.h)。测试假设 Linux 环境，可通过 make 启动：

```sh
make test
```

测试以 C 编写在 `tests` 目录的 `.toml` 文件中。开发功能或修复问题时可单独运行用例或测试组：

```sh
./scripts/test.py -l runners/test_runner  # 列出测试组
./scripts/test.py -L runners/test_runner test_dirs  # 列出测试用例
./scripts/test.py runners/test_runner test_dirs  # 运行指定测试组
```

断言失败时，test.py 尝试打印失败信息；以下保留实际输出格式：

```text
tests/test_dirs.toml:1:failure: test_dirs_root:1g12gg2 (PROG_SIZE=16, ERASE_SIZE=512) failed
tests/test_dirs.toml:5:assert: assert failed with 0, expected eq 42
    lfs_mount(&lfs, cfg) => 42;
```

可把其中的测试 ID 传给 test.py，仅运行该组参数组合：

```sh
./scripts/test.py runners/test_runner test_dirs_root:1g12gg2  # 运行指定参数组合
./scripts/test.py runners/test_runner test_dirs_root:1g12gg2 --gdb  # 失败时进入 gdb
```

其他常用参数：

```sh
./scripts/test.py runners/test_runner -b -j  # 并行测试
./scripts/test.py runners/test_runner -v -O-  # 将测试标准输出转发到当前标准输出
./scripts/test.py runners/test_runner -ddisk  # 保存结果磁盘镜像
./scripts/test.py --help  # 查看全部选项，也可使用 -h
```

## 许可证

littlefs 使用 [BSD-3-Clause]，完整内容见 [LICENSE.md](LICENSE.md)，贡献采用同一许可证。各文件以以下标记代替完整许可证文本，便于机器按 [SPDX 标识符](https://spdx.org/licenses/) 处理：

```text
SPDX-License-Identifier:    BSD-3-Clause
```

## 相关项目

- [littlefs-fuse]：通过 [FUSE] 在 Linux 直接挂载 littlefs，有助于使用 SD 卡调试。
- [littlefs-js]：JavaScript 封装，可用于[交互演示][littlefs-js-demo]。
- [littlefs-python]：Python 封装，可在 PC 创建文件系统镜像、评估适用性，生成待写入目标存储的镜像或检查已有二进制镜像。
- [littlefs-toy]：类似 tar 语法的镜像命令行工具，支持嵌在固件等其他文件中的 littlefs 镜像。
- [littlefs2-rust]：提供符合 Rust 使用习惯的 API，利用 Rust 内存安全等保证。
- [nim-littlefs]：Nim 封装/API，包含基于 littlefs-fuse 的 FUSE 实现。
- [chamelon]：面向 MirageOS 的纯 OCaml 实现，覆盖大部分 littlefs，可与参考实现互操作，但有部分限制。
- [littlefs-disk-img-viewer]：在浏览器查看 littlefs 磁盘镜像的低内存应用。
- [mklfs]：Lua RTOS 生态使用的镜像创建命令行工具。
- [mklittlefs]：ESP8266 和 RP2040 生态使用的镜像创建命令行工具。
- [pico-littlefs-usb]：通过 USB 模拟 FAT12，使宿主无需额外驱动即可挂载 littlefs。
- [ramcrc32bd]：使用 littlefs 的 32 位 CRC 纠错的块设备示例。
- [ramrsbd]：使用 Reed-Solomon 码纠错的块设备示例。
- [Mbed OS]：提供常见嵌入式存储块设备驱动，并以 [LittleFileSystem] 类提供 littlefs。
- [SPIFFS]：面向 NOR Flash 的日志文件系统，具有完整静态磨损均衡；对微控制器内部 Flash 等小容量存储可能性能更好。
- [Dhara]：面向小型 MCU 的 NAND Flash 转换层，每块及 RAM 只需固定的 _O(|address|)_ 指针结构，即可提供静态磨损均衡和抗断电能力。
- [ChaN's FatFs]：面向微控制器的轻量 FAT 实现。受 FAT 限制，不提供抗断电能力，但便于与 PC 互操作。

[BSD-3-Clause]: https://spdx.org/licenses/BSD-3-Clause.html
[littlefs-fuse]: https://github.com/geky/littlefs-fuse
[FUSE]: https://github.com/libfuse/libfuse
[littlefs-js]: https://github.com/geky/littlefs-js
[littlefs-js-demo]:http://littlefs.geky.net/demo.html
[littlefs-python]: https://pypi.org/project/littlefs-python/
[littlefs-toy]: https://github.com/tjko/littlefs-toy
[littlefs2-rust]: https://crates.io/crates/littlefs2
[nim-littlefs]: https://github.com/Graveflo/nim-littlefs
[chamelon]: https://github.com/yomimono/chamelon
[littlefs-disk-img-viewer]: https://github.com/tniessen/littlefs-disk-img-viewer
[mklfs]: https://github.com/whitecatboard/Lua-RTOS-ESP32/tree/master/components/mklfs/src
[mklittlefs]: https://github.com/earlephilhower/mklittlefs
[pico-littlefs-usb]: https://github.com/oyama/pico-littlefs-usb
[ramcrc32bd]: https://github.com/geky/ramcrc32bd
[ramrsbd]: https://github.com/geky/ramrsbd
[Mbed OS]: https://github.com/armmbed/mbed-os
[LittleFileSystem]: https://os.mbed.com/docs/mbed-os/latest/apis/littlefilesystem.html
[SPIFFS]: https://github.com/pellepl/spiffs
[Dhara]: https://github.com/dlbeer/dhara
[ChaN's FatFs]: http://elm-chan.org/fsw/ff/00index_e.html
