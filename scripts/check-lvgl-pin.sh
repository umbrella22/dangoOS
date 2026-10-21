#!/usr/bin/env bash
# Warn when ../lv_binding_rust is not at the revision recorded in lvgl.rev.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
want=$(tr -d '[:space:]' < lvgl.rev)
dir=../lv_binding_rust
if [[ ! -d $dir/.git ]]; then
    echo "check-lvgl-pin: $dir is missing; clone https://github.com/umbrella22/lvgl_binding_rust there" >&2
    exit 1
fi
have=$(git -C "$dir" rev-parse HEAD)
status=0
if [[ $have != "$want"* ]]; then
    echo "check-lvgl-pin: $dir is at ${have:0:7}, tested with $want" >&2
    status=1
fi
if [[ -n $(git -C "$dir" status --porcelain) ]]; then
    echo "check-lvgl-pin: $dir has uncommitted changes" >&2
    status=1
fi
[[ $status == 0 ]] && echo "check-lvgl-pin: $dir at $want, clean"
exit $status
