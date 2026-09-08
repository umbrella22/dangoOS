#!/usr/bin/env bash
# ESP32-S31 双变体构建脚本

set -e

echo "🔧 ESP32-S31 风扇控制系统 - 多变体构建工具"
echo "================================================"
echo ""

# 颜色定义
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

show_usage() {
    cat << EOF
用法: $0 [选项] <变体>

变体选择:
  mosaico      ESP-Mosaico 官方开发板 (480×480 AMOLED)
  korvo        ESP32-S31-Korvo-1 开发板 (800×480 RGB LCD)
  desktop      桌面开发模式（默认）

选项:
  --release    Release 构建（优化）
  --flash      构建后自动烧录
  --monitor    烧录后启动串口监视器

示例:
  $0 mosaico              # 开发构建 Mosaico
  $0 korvo --release      # Release 构建 Korvo-1
  $0 mosaico --flash      # 构建并烧录 Mosaico
  $0 desktop              # 桌面开发模式

EOF
}

# 参数解析
VARIANT=""
BUILD_TYPE="debug"
DO_FLASH=false
DO_MONITOR=false

while [[ $# -gt 0 ]]; do
    case $1 in
        mosaico|korvo|desktop)
            VARIANT="$1"
            shift
            ;;
        --release)
            BUILD_TYPE="release"
            shift
            ;;
        --flash)
            DO_FLASH=true
            shift
            ;;
        --monitor)
            DO_MONITOR=true
            shift
            ;;
        -h|--help)
            show_usage
            exit 0
            ;;
        *)
            echo "❌ 未知参数: $1"
            show_usage
            exit 1
            ;;
    esac
done

# 检查是否指定变体
if [ -z "$VARIANT" ]; then
    echo "❌ 错误: 必须指定变体"
    echo ""
    show_usage
    exit 1
fi

# 变体配置映射
case $VARIANT in
    mosaico)
        FEATURE="mosaico"
        CONFIG_NAME="ESP-Mosaico (480×480 AMOLED)"
        ;;
    korvo)
        FEATURE="korvo"
        CONFIG_NAME="ESP32-S31-Korvo-1 (800×480 RGB LCD)"
        ;;
    desktop)
        FEATURE=""
        CONFIG_NAME="桌面开发模式"
        ;;
esac

echo -e "${BLUE}📦 配置信息${NC}"
echo "  变体: $CONFIG_NAME"
echo "  构建类型: $BUILD_TYPE"
if [ "$DO_FLASH" = true ]; then
    echo "  烧录: 是"
fi
if [ "$DO_MONITOR" = true ]; then
    echo "  监视器: 是"
fi
echo ""

# 桌面模式直接构建
if [ "$VARIANT" = "desktop" ]; then
    echo -e "${GREEN}🔨 构建桌面版本...${NC}"
    if [ "$BUILD_TYPE" = "release" ]; then
        cargo build --release
    else
        cargo build
    fi
    echo ""
    echo -e "${GREEN}✅ 桌面版本构建完成！${NC}"
    echo "运行: cargo run"
    exit 0
fi

# ESP32 变体构建
echo -e "${GREEN}🔨 构建 ESP32-S31 固件 ($CONFIG_NAME)...${NC}"

# 切换到 ESP32 配置
if [ ! -f ".cargo/config.esp32.toml" ]; then
    echo -e "${YELLOW}⚠️  未找到 .cargo/config.esp32.toml，使用默认配置${NC}"
fi

# 构建命令
BUILD_CMD="cargo build"
if [ -n "$FEATURE" ]; then
    BUILD_CMD="$BUILD_CMD --features $FEATURE"
fi
if [ "$BUILD_TYPE" = "release" ]; then
    BUILD_CMD="$BUILD_CMD --release"
fi

echo "执行: $BUILD_CMD"
eval $BUILD_CMD

echo ""
echo -e "${GREEN}✅ 固件构建完成！${NC}"

# 烧录
if [ "$DO_FLASH" = true ]; then
    echo ""
    echo -e "${BLUE}📤 烧录固件到设备...${NC}"
    
    if [ "$BUILD_TYPE" = "release" ]; then
        BINARY="target/riscv32imafc-unknown-none-elf/release/fan-control"
    else
        BINARY="target/riscv32imafc-unknown-none-elf/debug/fan-control"
    fi
    
    if [ "$DO_MONITOR" = true ]; then
        espflash flash --monitor "$BINARY"
    else
        espflash flash "$BINARY"
    fi
fi

echo ""
echo -e "${GREEN}🎉 完成！${NC}"
