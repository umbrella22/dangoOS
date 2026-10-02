#!/usr/bin/env bash
# 分别检查主机和 MCU，避免为单一 target 构建整个 workspace。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

MCU_TARGET="riscv32imafc-unknown-none-elf"
cargo fmt --all -- --check
cargo test --locked -p fan-core
cargo test --locked -p fan-ui --lib
cargo test --locked -p fan-ui --features software-rendering --test software_rendering
cargo test --locked -p fan-ui --features embedded --test mosaic_rendering --test mosaic_visuals
cargo test --locked -p i-slint-renderer-software --lib
./scripts/test-touch.sh
./scripts/test-mosaic-runtime.sh
cargo test --locked -p fan-desktop
cargo check --locked -p fan-desktop
cargo build --locked -p fan-firmware --target "$MCU_TARGET" --release --bin fan-control-fw

# 仅看 MCU 的运行时依赖。Slint 构建脚本在主机运行，允许使用 std。
FAN_DEPS="$(mktemp "${TMPDIR:-/tmp}/fan-firmware-deps.XXXXXX")"
trap 'rm -f "$FAN_DEPS"' EXIT
cargo tree --locked -p fan-firmware --target "$MCU_TARGET" --edges normal --prefix none --format '{p}' > "$FAN_DEPS"
python3 - "$FAN_DEPS" <<'DEPENDENCIES'
from pathlib import Path
import sys

names = {line.split()[0] for line in Path(sys.argv[1]).read_text().splitlines() if line.strip()}
forbidden = {
    'fan-desktop', 'winit', 'i-slint-backend-winit', 'i-slint-backend-qt',
    'i-slint-backend-linuxkms', 'i-slint-backend-android-activity',
    'esp-idf-hal', 'esp-idf-sys', 'esp-idf-svc',
}
if unexpected := sorted(names & forbidden):
    raise SystemExit(f'固件包含非预期运行时依赖: {unexpected}')
print('检查通过：核心、启动时序、触摸协议与桌面模拟测试、桌面与 Mosaic 布局、主固件构建及固件运行时依赖隔离。')
DEPENDENCIES
