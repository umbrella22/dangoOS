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

SRC=crates/os/src
for file in platform/drivers/touch/protocol.rs platform/drivers/display/packing.rs \
    services/display/geometry.rs platform/drivers/nand/protocol.rs \
    platform/drivers/subboard/descriptor.rs platform/drivers/subboard/retry.rs \
    platform/drivers/subboard/signal.rs platform/drivers/subboard/touch_key.rs; do
    name="$(basename "$file" .rs)"
    rustc --edition=2021 --test "$SRC/$file" -o "$DIR/$name"
    "$DIR/$name" -q "$@"
done

# Motor admission/cancellation is pure policy; no device or ESP HAL required.
rustc --edition=2021 --test crates/os/src/services/feedback/haptic/policy.rs -o "$DIR/haptic-policy"
"$DIR/haptic-policy" -q "$@"

# Terminal LED clear acknowledgement gates the physical power cutoff.
rustc --edition=2021 --test crates/os/src/services/feedback/lights/shutdown.rs -o "$DIR/lights-shutdown"
"$DIR/lights-shutdown" -q "$@"
