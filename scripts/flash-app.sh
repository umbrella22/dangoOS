#!/usr/bin/env bash
# Build the firmware and write ONLY the main_app partition (0x210000).
#
# Never touches the bootloader, the partition table or vibe_mode at 0x20000.
# Put the board in download mode first (hold BOOT, tap RESET).
#
# Usage: scripts/flash-app.sh <port> [image-name]
#   scripts/flash-app.sh /dev/cu.usbmodem113401
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

PORT="${1:?usage: scripts/flash-app.sh <port> [image-name]}"
NAME="${2:-mosaico-s31-fw}"
ELF=target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw
IMAGE="target/${NAME}.bin"
MAIN_APP=0x210000

cargo fw --offline
espflash save-image --chip esp32s31 "$ELF" "$IMAGE"
shasum -a 256 "$IMAGE"

esptool --chip esp32s31 --port "$PORT" --baud 115200 \
    --before no-reset --after hard-reset \
    write-flash "$MAIN_APP" "$IMAGE"
