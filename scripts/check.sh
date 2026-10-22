#!/usr/bin/env bash
# Everything that can be checked without a board.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Library: behaviour, fox renderer, UI tree, telemetry.
cargo test -p mosaico-s31 --offline

# Pure driver modules (the bin itself has no host tests).
./scripts/test-drivers.sh

# Host previews must still run.
cargo run -p mosaico-s31 --example preview --offline >/dev/null
cargo run -p mosaico-s31 --example splash_preview --offline >/dev/null

# Firmware for the device target.
cargo fw --offline
