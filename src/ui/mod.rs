// UI 模块：根据编译时的硬件变体选择对应的 UI 布局
//
// 树摇优化：编译时只包含目标硬件的 UI 定义
// - mosaico: 480×480 方屏 (fan_control.slint)
// - korvo: 800×480 宽屏 (fan_control_800x480.slint)

// 编译时元数据（用于调试）
#[cfg(feature = "mosaico")]
pub const VARIANT_NAME: &str = "ESP-Mosaico (480×480)";

#[cfg(feature = "korvo")]
pub const VARIANT_NAME: &str = "Korvo-1 (800×480)";

#[cfg(not(any(feature = "mosaico", feature = "korvo")))]
pub const VARIANT_NAME: &str = "Desktop (480×480)";

