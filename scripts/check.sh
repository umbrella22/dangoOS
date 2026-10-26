#!/usr/bin/env bash
# Everything that can be checked without a board.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Library: behaviour, fox renderer, UI tree, telemetry.
cargo test -p mosaico-s31 --offline

# File system: littlefs + VFS, built with the host C compiler.
cargo test -p mosaico-fs --offline

# Lua runtime: vendored Lua 5.5.1 + glue, built with the host C compiler.
cargo test -p mosaico-lua --offline

# Real LVGL clean/rebuild publication and pointer transaction boundaries.
cargo run --offline -p mosaico-s31 --example n5_render_boundary_probe
cargo run --offline -p mosaico-s31 --example n5_native_input_probe

# Pure driver modules (the bin itself has no host tests).
./scripts/test-drivers.sh

# Host previews must still run.
cargo run -p mosaico-s31 --example preview --offline >/dev/null
cargo run -p mosaico-s31 --example splash_preview --offline >/dev/null

# Firmware for the device target.
cargo fw --offline

# Vendored C must not collide with symbols the S31 ROM scripts assign.
./scripts/check-rom-symbols.sh
