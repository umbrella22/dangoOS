#!/usr/bin/env bash
# Report C symbols we define that the ESP32-S31 ROM linker scripts also define.
#
# Those scripts assign ROM addresses with plain `name = 0x...;` (not PROVIDE),
# which overrides our own definition at link time: calls silently go to the
# ROM copy. Run after a firmware build (`cargo fw`); exits 1 on a collision.
set -euo pipefail
cd "$(dirname "$0")/../.."

ROM=$(ls -d ~/.cargo/git/checkouts/esp-hal-*/*/esp-rom-sys/ld/esp32s31/rom 2>/dev/null | head -1)
[ -n "$ROM" ] || { echo "esp-rom-sys ROM scripts not found" >&2; exit 2; }
NM=${NM:-$(command -v riscv32-esp-elf-nm || echo ~/esp-tools/riscv32-esp-elf/bin/riscv32-esp-elf-nm)}
BUILD=target/riscv32imafc-unknown-none-elf/release/build

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sed -nE 's/^[[:space:]]*(PROVIDE[[:space:]]*\([[:space:]]*)?([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=.*/\2/p' \
    "$ROM"/*.ld | sort -u > "$tmp/rom"
libs=$(ls "$BUILD"/fs-*/out/*.a "$BUILD"/lua-engine-*/out/*.a 2>/dev/null || true)
[ -n "$libs" ] || { echo "no C libraries under $BUILD; run cargo fw first" >&2; exit 2; }
for a in $libs; do "$NM" --defined-only -g "$a" 2>/dev/null | awk 'NF==3{print $3}'; done \
    | sort -u > "$tmp/ours"

clash=$(comm -12 "$tmp/rom" "$tmp/ours")
if [ -n "$clash" ]; then
    echo "symbols also defined by the S31 ROM (rename them in build.rs):"
    echo "$clash" | sed 's/^/  /'
    exit 1
fi
echo "rom symbols: no collisions ($(wc -l < "$tmp/ours" | tr -d ' ') C symbols checked)"
