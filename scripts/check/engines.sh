#!/usr/bin/env bash
# The OS consumes precompiled EmbedTS inputs; all source compilation is external.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
OUT=target/engine-checks
mkdir -p "$OUT"
python3 scripts/check/compiled_apps.py
cargo test -p os --no-default-features --features ts-engine --locked --offline
cargo test -p app-pack --no-default-features --features ts-engine --locked --offline
for selection in neither both; do
    ENGINE_FEATURES=''
    if [ "$selection" = both ]; then ENGINE_FEATURES=lua-engine,ts-engine; fi
    if cargo check -p os --no-default-features --features "$ENGINE_FEATURES" --locked --offline > "$OUT/$selection.log" 2>&1; then
        echo "invalid engine selection succeeded: $selection" >&2
        exit 1
    fi
done
for engine in ts lua; do
    cargo build -p os --bin os-fw --no-default-features --features "firmware,dual-core-ui,$engine-engine" --release --target riscv32imafc-unknown-none-elf --locked --offline
    cp target/riscv32imafc-unknown-none-elf/release/os-fw "$OUT/$engine.elf"
    cargo tree -p os --no-default-features --features "firmware,dual-core-ui,$engine-engine" --target riscv32imafc-unknown-none-elf --edges normal --prefix none --format '{p} features=[{f}]' --locked --offline > "$OUT/$engine-dependencies.txt"
done
ENGINE_NM=${NM:-$(command -v riscv32-esp-elf-nm || echo ~/esp-tools/riscv32-esp-elf/bin/riscv32-esp-elf-nm)}
python3 scripts/check/resource_budget.py --elf "$OUT/ts.elf" --output "$OUT/ts-resource-budget.json"
python3 scripts/check/engine_artifacts.py --out "$OUT" --nm "$ENGINE_NM"
