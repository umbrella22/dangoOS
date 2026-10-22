# Thread compatibility fixtures

The original `harness.lua`, `blocking_recv.lua`, `child_a.lua`, `child_b.lua`,
`consumer.lua`, `echo_child.lua`, `lock_holder.lua`, `lock_waiter.lua`,
`producer.lua` and `sync_echo.lua` are copied unchanged from
`https://github.com/espressif/esp-claw`, revision
`345073de68179542c307e303df81e38baadf77f0`, directory
`components/lua_modules/lua_module_thread/test_apps/thread_smoke_test/main/lua`.
Apache-2.0; see the included license.

`hostile_worker.lua` is a local regression fixture for capability isolation,
binary queues, lock ownership, stopping and hostile Lua finalizers.
