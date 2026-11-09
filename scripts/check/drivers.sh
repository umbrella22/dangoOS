#!/usr/bin/env bash
# Host tests for the pure driver modules.
#
# The firmware bin is built with `test = false` (it needs the RISC-V target),
# so these dependency-free files are compiled standalone with `rustc --test`:
# the CST9220 report decoder, the RGB565 window packer, the panel geometry,
# the slot EEPROM descriptor parser and the slot input filters.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

DIR="$(mktemp -d "${TMPDIR:-/tmp}/mosaico-driver-tests.XXXXXX")"
trap 'rm -rf "$DIR"' EXIT

for file in crates/board-s31/src/touch/protocol.rs crates/board-s31/src/display/packing.rs \
    crates/board-s31/src/geometry.rs crates/board-s31/src/nand/protocol.rs \
    crates/os/src/platform/drivers/subboard/descriptor.rs \
    crates/os/src/platform/drivers/subboard/retry.rs \
    crates/os/src/platform/drivers/subboard/signal.rs \
    crates/os/src/platform/drivers/subboard/touch_key.rs; do
    name="$(basename "$file" .rs)"
    rustc --edition=2021 --test "$file" -o "$DIR/$name"
    "$DIR/$name" -q "$@"
done

# Motor admission/cancellation is pure policy; no device or ESP HAL required.
rustc --edition=2021 --test crates/os/src/services/feedback/haptic/policy.rs -o "$DIR/haptic-policy"
"$DIR/haptic-policy" -q "$@"

# Terminal LED clear acknowledgement gates the physical power cutoff.
rustc --edition=2021 --test crates/os/src/services/feedback/lights/shutdown.rs -o "$DIR/lights-shutdown"
"$DIR/lights-shutdown" -q "$@"
