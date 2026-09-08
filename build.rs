fn main() {
    // 根据 feature 选择不同的 UI 文件（树摇优化）
    #[cfg(feature = "s31-variant-a")]
    let ui_file = "src/ui/fan_control.slint";
    
    #[cfg(feature = "s31-variant-b")]
    let ui_file = "src/ui/fan_control_800x480.slint";
    
    #[cfg(not(any(feature = "s31-variant-a", feature = "s31-variant-b")))]
    let ui_file = "src/ui/fan_control.slint";
    
    // 编译选定的 Slint UI 文件
    slint_build::compile(ui_file).unwrap();
    
    // 如果任一 UI 文件改变则重新编译
    println!("cargo:rerun-if-changed=src/ui/fan_control.slint");
    println!("cargo:rerun-if-changed=src/ui/fan_control_800x480.slint");
    println!("cargo:rerun-if-changed=src/embedded_hal/board.rs");
}
