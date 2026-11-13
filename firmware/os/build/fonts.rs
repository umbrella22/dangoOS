//! Build the external font set and its small, storage-independent recovery set.
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

const NAMES: [&str; 4] = ["20", "24", "30", "digits-64"];
const RECOVERY_TEXT: &str = "低电量即将关机，请连接电源";

pub fn generate() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let mut bundle = b"MUIF\x01\x00\x04\x00".to_vec();
    let fonts: Vec<Vec<u8>> = NAMES
        .iter()
        .map(|name| {
            let path = root.join(format!("src/assets/hub-{name}.bin"));
            println!("cargo:rerun-if-changed={}", path.display());
            fs::read(path).unwrap()
        })
        .collect();
    for font in &fonts {
        bundle.extend_from_slice(&(font.len() as u32).to_le_bytes());
    }
    for (name, font) in NAMES.iter().zip(&fonts) {
        assert_eq!(&font[..6], b"MFNT\x01\x04");
        bundle.extend_from_slice(font);
        // Copy exact raster data and metrics; regeneration needs no TTF/Pillow.
        let count = u16::from_le_bytes(font[10..12].try_into().unwrap()) as usize;
        let bitmap_start = 16 + count * 16;
        let mut table = Vec::new();
        let mut bitmaps = Vec::new();
        for entry in font[16..bitmap_start].chunks_exact(16) {
            let cp = u32::from_le_bytes(entry[..4].try_into().unwrap());
            let ch = char::from_u32(cp).expect("font codepoint");
            if ch.is_ascii() || RECOVERY_TEXT.contains(ch) {
                let offset = u32::from_le_bytes(entry[4..8].try_into().unwrap()) as usize;
                let len = (entry[10] as usize).div_ceil(2) * entry[11] as usize;
                let mut row = entry.to_vec();
                row[4..8].copy_from_slice(&(bitmaps.len() as u32).to_le_bytes());
                table.extend_from_slice(&row);
                bitmaps
                    .extend_from_slice(&font[bitmap_start + offset..bitmap_start + offset + len]);
            }
        }
        let mut recovery = font[..16].to_vec();
        recovery[10..12].copy_from_slice(&((table.len() / 16) as u16).to_le_bytes());
        recovery.extend_from_slice(&table);
        recovery.extend_from_slice(&bitmaps);
        // A complete small text size preserves readable native settings when
        // app-only OTA leaves an older /system without the new resource set.
        fs::write(
            out.join(format!("recovery-{name}.bin")),
            if *name == "20" { font } else { &recovery },
        )
        .unwrap();
    }
    let hash: [u8; 32] = Sha256::digest(&bundle).into();
    let id: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    fs::write(out.join("ui-fonts.bin"), &bundle).unwrap();
    fs::write(
        out.join("ui-fonts.rs"),
        format!(
            "pub const SHA256: [u8; 32] = {hash:?};\n\
         pub const ID: &str = \"{id}\";\n\
         pub const STAMP: &str = \"DANGO_UI_FONTS_V1:{id}:{};\";\n\
         pub const BYTES: usize = {};\n\
         pub const FILENAME: &str = \"fonts-{id}.bin\";\n\
         pub const DATA_PATH: &str = \"/data/ui/fonts-{id}.bin\";\n\
         pub const SYSTEM_PATH: &str = \"/system/ui/fonts-{id}.bin\";\n",
            bundle.len(),
            bundle.len()
        ),
    )
    .unwrap();
}
