#!/usr/bin/env bash
# Build and install main through the independent Recovery protocol.
# Enter Recovery from main first: console.py recovery --port <runtime-port>.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
PORT="${1:?Recovery USB port required}"
IMAGE="target/main.bin"
cargo fw --offline
espflash save-image --chip esp32s31 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
    target/riscv32imafc-unknown-none-elf/release/os-fw "$IMAGE"
python3 tools/device/recovery.py --port "$PORT" install "$IMAGE" --boot
