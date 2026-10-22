//! Validate a packed skin with the production parser, render it and optionally
//! write RGB565 pixels for comparison. No device or global VFS is used.
use mosaico_s31::fox::{skin::Skin, FoxPose, FoxRenderer, Scene, PIXELS};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = args
        .get(1)
        .expect("usage: skin_check fox.bin [output.rgb565]");
    let skin = Skin::parse(std::fs::read(path).expect("read skin")).expect("validate skin");
    let mut out = vec![0; PIXELS];
    FoxRenderer::new().render(&Scene::with_skin(FoxPose::Idle, skin), &mut out);
    let bytes = out.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<_>>();
    if let Some(path) = args.get(2) {
        std::fs::write(path, &bytes).expect("write pixels");
    }
    println!(
        "skin PASS: {} pixels, render CRC32={:08x}",
        PIXELS,
        mosaico_s31::hub::push::crc32(&bytes)
    );
}
