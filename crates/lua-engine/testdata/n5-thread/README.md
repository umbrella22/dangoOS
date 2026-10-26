# 线程兼容性测试样本

原始 `harness.lua`、`blocking_recv.lua`、`child_a.lua`、`child_b.lua`、
`consumer.lua`、`echo_child.lua`、`lock_holder.lua`、`lock_waiter.lua`、
`producer.lua` 和 `sync_echo.lua` 均原样复制自
[espressif/esp-claw](https://github.com/espressif/esp-claw)，提交为
`345073de68179542c307e303df81e38baadf77f0`，上游目录为
`components/lua_modules/lua_module_thread/test_apps/thread_smoke_test/main/lua`。
许可证为 Apache-2.0，见本目录的 `LICENSE`。

`hostile_worker.lua` 是本地回归样本，覆盖能力隔离、二进制队列、锁归属、停止和恶意
Lua 终结器行为。
