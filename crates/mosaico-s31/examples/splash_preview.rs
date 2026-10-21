//! Headless frames of the boot splash and the fox entrance, no LVGL needed.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p mosaico-s31 --example splash_preview --offline
//! ```
//!
//! Writes PPM frames under `target/splash-preview/`:
//!
//! - `splash-<ms>.ppm`: the `ikaros` wordmark timeline (360x120);
//! - `entrance-<ms>.ppm`: the fox drop, squash and hop at canvas size.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use mosaico_s31::fox::{self, entrance, FoxAnimator, FoxInput, FoxPose, FoxRenderer};
use mosaico_s31::ui::splash::{self, SPLASH_HEIGHT, SPLASH_WIDTH};
use mosaico_s31::Expression;

fn main() {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/splash-preview");
    if output.exists() {
        fs::remove_dir_all(&output).expect("clear previous output");
    }
    fs::create_dir_all(&output).expect("create output directory");
    let output = output.canonicalize().expect("canonical output path");

    write_splash_frames(&output);
    write_entrance_frames(&output);
    println!("preview frames in {}", output.display());
}

/// The wordmark timeline: reveal, underline, hold, fade.
fn write_splash_frames(output: &Path) {
    let mut buffer = vec![0u16; SPLASH_WIDTH * SPLASH_HEIGHT];
    let underline_done_ms = (splash::WORDMARK.len() as u64 - 1) * splash::LETTER_STAGGER_MS
        + splash::LETTER_REVEAL_MS
        + splash::UNDERLINE_MS;
    let mut previous = None;
    for ms in [0u64, 120, 260, 480, 700, 900, 1_000, 1_050, 1_340] {
        let finished = splash::render(&mut buffer, ms);
        let path = output.join(format!("splash-{ms:04}.ppm"));
        write_ppm_rgb565(&path, &buffer, SPLASH_WIDTH, SPLASH_HEIGHT);
        let hash = fnv1a(&buffer);
        println!("splash {ms:04}ms: finished={finished} hash=0x{hash:016x}");
        // Every step before the underline completes must change the frame;
        // the hold afterwards is static on purpose.
        if let Some(last) = previous {
            if ms <= underline_done_ms {
                assert_ne!(hash, last, "each reveal step must differ");
            }
        }
        previous = Some(hash);
    }
}

/// The entrance: the same input the widget composes, without the widget.
fn write_entrance_frames(output: &Path) {
    let mut animator = FoxAnimator::new(FoxPose::from(Expression::Idle));
    let mut renderer = FoxRenderer::new();
    let mut canvas = vec![0u16; fox::PIXELS];
    let mut last_ms = 0u64;
    for ms in [0u64, 120, 240, 380, 440, 520, 620, 760, 830, 900] {
        animator.advance((ms - last_ms) as u32);
        last_ms = ms;
        let pose = entrance::pose(ms);
        let input = FoxInput {
            scale_x: pose.scale_x,
            scale_y: pose.scale_y,
            offset_y: pose.offset_y,
            ..FoxInput::REST
        };
        renderer.render(&animator.scene(input), &mut canvas);
        let path = output.join(format!("entrance-{ms:04}.ppm"));
        write_ppm_rgb565(&path, &canvas, fox::W, fox::H);
        println!(
            "entrance {ms:04}ms: offset_y={} scale=({:.3}, {:.3})",
            pose.offset_y, pose.scale_x, pose.scale_y
        );
    }
}

fn write_ppm_rgb565(path: &Path, pixels: &[u16], width: usize, height: usize) {
    let mut bytes = Vec::with_capacity(width * height * 3);
    for &pixel in pixels {
        let r = ((pixel >> 11) & 0x1f) as u8;
        let g = ((pixel >> 5) & 0x3f) as u8;
        let b = (pixel & 0x1f) as u8;
        bytes.extend_from_slice(&[
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ]);
    }
    let mut file = fs::File::create(path).expect("create PPM");
    write!(file, "P6\n{width} {height}\n255\n").expect("write PPM header");
    file.write_all(&bytes).expect("write PPM pixels");
}

fn fnv1a(pixels: &[u16]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in pixels.iter().flat_map(|pixel| pixel.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
