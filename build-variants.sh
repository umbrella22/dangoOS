#!/usr/bin/env bash
# Workspace 构建入口；仅 --flash 会写入设备。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

show_usage() {
    cat <<'USAGE'
用法: ./build-variants.sh [desktop|mosaico] [选项]

  desktop     构建 480×480 桌面预览（默认）
  mosaico     构建 ESP-Mosaico (v1.2) 固件，固定使用 release

选项:
  --release   桌面使用 release；固件默认已启用
  --flash     固件构建成功后烧录
  --monitor   配合 --flash 启动串口监视器
USAGE
}

VARIANT=""
DESKTOP_RELEASE=false
DO_FLASH=false
DO_MONITOR=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        desktop|mosaico)
            if [[ -n "$VARIANT" ]]; then
                echo "一次只能选择一个构建目标" >&2
                exit 1
            fi
            VARIANT="$1"
            ;;
        --release) DESKTOP_RELEASE=true ;;
        --flash) DO_FLASH=true ;;
        --monitor) DO_MONITOR=true ;;
        -h|--help) show_usage; exit 0 ;;
        *) echo "未知参数: $1" >&2; show_usage; exit 1 ;;
    esac
    shift
done
VARIANT="${VARIANT:-desktop}"

if [[ "$DO_MONITOR" == true && "$DO_FLASH" != true ]]; then
    echo "--monitor 必须配合 --flash 使用" >&2
    exit 1
fi

case "$VARIANT" in
    desktop)
        if [[ "$DO_FLASH" == true ]]; then
            echo "桌面程序不能烧录；真机请选择 mosaico" >&2
            exit 1
        fi
        ARGS=(build --locked -p fan-desktop)
        if [[ "$DESKTOP_RELEASE" == true ]]; then ARGS+=(--release); fi
        cargo "${ARGS[@]}"
        echo "桌面构建完成；运行: cargo desktop"
        ;;
    mosaico)
        # QSPI 整帧刷新依赖实时传输性能，固件使用已验证的优化配置。
        cargo firmware --locked
        ELF="${CARGO_TARGET_DIR:-target}/riscv32imafc-unknown-none-elf/release/fan-control-fw"
        echo "固件构建完成: $ELF"
        if [[ "$DO_FLASH" == true ]]; then
            ARGS=(flash --chip esp32s31)
            if [[ "$DO_MONITOR" == true ]]; then ARGS+=(--monitor); fi
            espflash "${ARGS[@]}" "$ELF"
        fi
        ;;
esac
