#!/usr/bin/env bash
# Host regression tests, repository app validation and the device build.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
python3 scripts/assets/gen_hub_font.py --check
cargo test --workspace --locked --offline
cargo run -p app-pack --locked --offline -- check apps
cargo run -p app-pack --locked --offline -- skins dango-skin
python3 -m unittest discover -s tests/host
./scripts/check/drivers.sh
cargo fw --locked --offline
./scripts/check/rom-symbols.sh
