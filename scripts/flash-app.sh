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
# The last 2 MiB of main_app (0x7E0000..0x9E0000) hold the /data file system
# (see crates/mosaico-s31/src/drivers/storage.rs); the image must end before.
IMAGE_MAX=$((0x7E0000 - MAIN_APP))

cargo fw --offline
espflash save-image --chip esp32s31 "$ELF" "$IMAGE"
shasum -a 256 "$IMAGE"
SIZE=$(wc -c < "$IMAGE" | tr -d ' ')
if [ "$SIZE" -gt "$IMAGE_MAX" ]; then
    echo "error: $IMAGE is $SIZE bytes; it would overwrite /data (limit $IMAGE_MAX)" >&2
    exit 1
fi
echo "image $SIZE bytes of $IMAGE_MAX ($((SIZE * 100 / IMAGE_MAX))%)"

esptool --chip esp32s31 --port "$PORT" --baud 115200 \
    --before no-reset --after hard-reset \
    write-flash "$MAIN_APP" "$IMAGE"
