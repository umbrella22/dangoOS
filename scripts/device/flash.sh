#!/usr/bin/env bash
# Explicit ROM recovery of slot zero. Normal A/B updates use console.py ota.
# This does not change otadata: selected slot one may continue booting afterward.
# Usage: scripts/device/flash.sh --rom-recovery-slot0 <port> [image-name]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
if [ "${1:-}" != "--rom-recovery-slot0" ]; then
    echo 'Refusing unconditional slot-zero flash. Use console.py ota for A/B updates.' >&2
    echo 'Manual recovery: flash.sh --rom-recovery-slot0 <port> [image-name]' >&2
    exit 2
fi
shift
PORT="${1:?port required}"
NAME="${2:-os-fw}"
ELF=target/riscv32imafc-unknown-none-elf/release/os-fw
IMAGE="target/${NAME}.bin"
IMAGE_MAX=$((0x600000))
cargo fw --offline
espflash save-image --chip esp32s31 --flash-size 16mb --flash-mode dio --flash-freq 40mhz "$ELF" "$IMAGE"
python3 - "$IMAGE" <<'PY'
import sys
sys.path.insert(0, 'scripts/device')
from ota_image import read_image
read_image(sys.argv[1], require_ab=True)
PY
shasum -a 256 "$IMAGE"
SIZE=$(wc -c < "$IMAGE" | tr -d ' ')
if [ "$SIZE" -gt "$IMAGE_MAX" ]; then
    echo "error: image $SIZE exceeds 6 MiB slot ($IMAGE_MAX)" >&2
    exit 1
fi
echo "ROM recovery: slot zero at 0x210000, $SIZE bytes; otadata selection is unchanged."
esptool --chip esp32s31 --port "$PORT" --baud 115200 \
    --before no-reset --after hard-reset \
    write-flash --flash-size 16MB --flash-mode dio --flash-freq 40m 0x210000 "$IMAGE"
