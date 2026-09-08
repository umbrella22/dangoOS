// 服务层模块 - 硬件抽象和实现

pub mod hardware;  // 硬件抽象层（trait 定义）
pub mod mock;      // 模拟硬件实现（开发用）
pub mod esp32;     // 真机硬件实现（待完成）
pub mod animation; // 动画控制
