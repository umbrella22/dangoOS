#!/usr/bin/env bash
# Host-test the production scheduling/retry policy and game without ESP HAL.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
MOSAIC_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mosaic-runtime-tests.XXXXXX")"
trap 'rm -rf "$MOSAIC_TEST_DIR"' EXIT
rustc --edition=2021 --test crates/fan-firmware/src/runtime.rs -o "$MOSAIC_TEST_DIR/runtime"
"$MOSAIC_TEST_DIR/runtime" "$@"
# Host math wrapper for the production game; MCU still uses locked no_std libm.
cat > "$MOSAIC_TEST_DIR/game.rs" <<TESTS
extern crate self as libm;
pub fn sqrtf(v: f32) -> f32 { v.sqrt() }
#[path = "$(pwd)/crates/fan-firmware/src/drivers/breakout.rs"] mod breakout;
TESTS
rustc --edition=2021 --test "$MOSAIC_TEST_DIR/game.rs" -o "$MOSAIC_TEST_DIR/game"
"$MOSAIC_TEST_DIR/game" "$@"
rustc --edition=2021 --test crates/fan-firmware/src/drivers/imu/protocol.rs -o "$MOSAIC_TEST_DIR/imu"
"$MOSAIC_TEST_DIR/imu" "$@"

rustc --edition=2021 --test crates/fan-firmware/src/drivers/camera/protocol.rs -o "$MOSAIC_TEST_DIR/camera"
"$MOSAIC_TEST_DIR/camera" "$@"
