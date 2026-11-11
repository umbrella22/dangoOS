#!/usr/bin/env bash
# Host regression tests, repository app validation and the device build.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
python3 scripts/assets/gen_hub_font.py --check
cargo test --workspace --locked --offline
cargo test -p os --features ui-validation --test ui_reliability --locked --offline -- --test-threads=1
cargo run -p app-pack --locked --offline -- check apps
cargo run -p app-pack --locked --offline -- skins resources/skins
python3 -m unittest discover -s tests/host
./scripts/check/drivers.sh
./scripts/check/engines.sh
cargo recovery --locked --offline
python3 scripts/check/resource_budget.py --output target/resource-budget.json
./scripts/check/rom-symbols.sh
