#!/usr/bin/env bash
# Host regression tests, repository app validation and the device build.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo test --workspace --offline
cargo run -p app-pack --offline -- check apps
python3 -m unittest discover -s tests/host
./scripts/check/drivers.sh
cargo fw --offline
./scripts/check/rom-symbols.sh
