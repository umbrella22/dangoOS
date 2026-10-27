#!/usr/bin/env bash
# Host regression tests, repository app validation and the device build.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
cargo test --workspace --locked --offline
cargo run -p app-pack --locked --offline -- check apps
python3 -m unittest discover -s tests/host
./scripts/check/drivers.sh
cargo fw --locked --offline
./scripts/check/rom-symbols.sh
