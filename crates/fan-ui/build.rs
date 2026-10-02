fn main() {
    // 按消费者选择 UI 根, 避免两套 UI 同时编进固件(.text 翻倍, ROM 溢出):
    //   桌面(std)     → ui/fan_control.slint(旧风扇界面, 模拟器用)
    //   固件(embedded)→ ui/mosaic/mosaic.slint(官方 Hub UI)
    let layout = if std::env::var_os("CARGO_FEATURE_EMBEDDED").is_some() {
        "ui/mosaic/mosaic.slint"
    } else {
        "ui/fan_control.slint"
    };
    let mut config = slint_build::CompilerConfiguration::new();
    if std::env::var_os("CARGO_FEATURE_SOFTWARE_RENDERING").is_some() {
        config = config.embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    }
    slint_build::compile_with_config(layout, config).expect("compile Slint layout");
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=assets/fonts");
}
