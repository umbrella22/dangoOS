#!/usr/bin/env bash
# 保留原入口；验证实际 crate / feature 边界及编译结果。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
exec ./scripts/check.sh "$@"
