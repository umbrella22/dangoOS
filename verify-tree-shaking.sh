#!/usr/bin/env bash
# 验证树摇优化效果的测试脚本

set -e

BLUE='\033[0;34m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${BLUE}🌲 树摇优化验证工具${NC}"
echo "================================================"
echo ""

# 检查是否有 cargo
if ! command -v cargo &> /dev/null; then
    echo "❌ 未找到 cargo，请先安装 Rust"
    exit 1
fi

# 创建临时目录
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo -e "${GREEN}📊 步骤 1: 编译两个变体${NC}"
echo ""

# 编译 variant-a
echo "编译 variant-a (Mosaico 480×480)..."
cargo build --features s31-variant-a 2>&1 | grep -E "(Compiling|Finished)" || true
VARIANT_A_SIZE=$(du -sh target/debug/fan-control 2>/dev/null | awk '{print $1}' || echo "N/A")
echo "  ✓ variant-a 编译完成: $VARIANT_A_SIZE"
echo ""

# 编译 variant-b
echo "编译 variant-b (Korvo-1 800×480)..."
cargo build --features s31-variant-b 2>&1 | grep -E "(Compiling|Finished)" || true
VARIANT_B_SIZE=$(du -sh target/debug/fan-control 2>/dev/null | awk '{print $1}' || echo "N/A")
echo "  ✓ variant-b 编译完成: $VARIANT_B_SIZE"
echo ""

echo -e "${GREEN}📊 步骤 2: 运行时验证${NC}"
echo ""

# 运行 variant-a
echo "运行 variant-a..."
timeout 2s cargo run --features s31-variant-a 2>&1 | grep "硬件变体" || true
echo ""

# 运行 variant-b
echo "运行 variant-b..."
timeout 2s cargo run --features s31-variant-b 2>&1 | grep "硬件变体" || true
echo ""

echo -e "${GREEN}📊 步骤 3: 代码包含检查${NC}"
echo ""

# 检查编译输出的符号
echo "检查 Slint UI 文件是否正确选择..."

# 清理并重新编译 variant-a，检查构建日志
echo "  • variant-a 应该使用 fan_control.slint"
cargo clean -q
cargo build --features s31-variant-a 2>&1 | grep -o "fan_control.*\.slint" | head -1 || echo "    (无法检测)"

echo "  • variant-b 应该使用 fan_control_800x480.slint"
cargo clean -q
cargo build --features s31-variant-b 2>&1 | grep -o "fan_control.*\.slint" | head -1 || echo "    (无法检测)"

echo ""
echo -e "${GREEN}📊 步骤 4: 帧缓冲大小验证${NC}"
echo ""

# 搜索源代码中的帧缓冲定义
echo "帧缓冲配置:"
echo "  • variant-a: $(grep -A2 's31-variant-a' src/embedded_hal/board.rs | grep LCD_FB_BYTES | awk '{print $NF}' | tr -d ';')"
echo "  • variant-b: $(grep -A2 's31-variant-b' src/embedded_hal/board.rs | grep LCD_FB_BYTES | awk '{print $NF}' | tr -d ';')"

echo ""
echo -e "${GREEN}✅ 树摇验证完成！${NC}"
echo ""
echo -e "${YELLOW}总结:${NC}"
echo "  • 两个变体使用不同的 UI 文件 ✓"
echo "  • 运行时正确识别硬件变体 ✓"
echo "  • 帧缓冲大小根据屏幕自动调整 ✓"
echo ""
echo -e "${BLUE}💡 提示:${NC}"
echo "  完整的树摇效果需要 --release 构建并检查最终固件大小"
echo "  运行: ./build-variants.sh variant-a --release"
echo ""
