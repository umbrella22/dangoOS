# 脚本引擎对比的宿主适配

`runner/` 分别链接 Lua 或外部 EmbedTS 引擎，测量真实 VM 执行与内存。它不包含 TS 源码或编译器。工作负载、校验和参考实现、报告器和协议测试位于独立 EmbedTypescript 仓库的 `tests/benchmarks/`。

在该仓库运行 `python3 tests/benchmarks/bench.py host target/bench --profile host --dango-root ../esp-32`，通过显式 OS checkout 提供 Lua 引擎、存储/图形宿主和应用打包器。
