// UI 模块：根据编译时的硬件变体选择对应的 UI 布局
//
// 树摇优化：编译时只包含目标硬件的 UI 定义
// - s31-variant-a: 480×480 方屏 (fan_control.slint)
// - s31-variant-b: 800×480 宽屏 (fan_control_800x480.slint)

// 编译时元数据（用于调试）
#[cfg(feature = "s31-variant-a")]
pub const VARIANT_NAME: &str = "ESP-Mosaico (480×480)";

#[cfg(feature = "s31-variant-b")]
pub const VARIANT_NAME: &str = "Korvo-1 (800×480)";

#[cfg(not(any(feature = "s31-variant-a", feature = "s31-variant-b")))]
pub const VARIANT_NAME: &str = "Desktop (480×480)";

