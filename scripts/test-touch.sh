#!/usr/bin/env bash
# 测试固件实际使用的 CST9220 报告解析/双字节寄存器命令，含无效帧保留输入状态。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TOUCH_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fan-touch-tests.XXXXXX")"
trap 'rm -rf "$TOUCH_TEST_DIR"' EXIT
rustc --edition=2021 --test crates/fan-firmware/src/drivers/touch/protocol.rs -o "$TOUCH_TEST_DIR/touch-protocol"
"$TOUCH_TEST_DIR/touch-protocol" "$@"
