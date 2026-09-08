fn main() {
    // 编译 Slint UI 文件
    slint_build::compile("src/ui/fan_control.slint").unwrap();
    
    // 如果文件改变则重新编译
    println!("cargo:rerun-if-changed=src/ui/fan_control.slint");
}
