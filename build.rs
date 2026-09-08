fn main() {
    // no_std 目标:资源全部内嵌进固件,不依赖文件系统
    // EmbedForSoftwareRenderer:以软件渲染器优化的格式内嵌图片与字体(slint 1.17 起)
    slint_build::compile_with_config(
        "ui/appwindow.slint",
        slint_build::CompilerConfiguration::new()
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("编译 ui/appwindow.slint 失败");
}
