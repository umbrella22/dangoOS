//! Headless preview of the boot splash, the entrance drop and the cat skin.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p mosaico-s31 --example splash_preview --offline
//! ```
//!
//! Writes PPM frames under `target/splash-preview/`:
//!
//! - `splash-<ms>.ppm` — the `ikaros` wordmark timeline;
//! - `entrance-<ms>.ppm` — the ball drop/settle/hop, composited at 480x480;
//! - `skin-cat.ppm`, `skin-fox.ppm` — the character skins.

use core::ptr;
use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use grok_ball::raster::{render_sized_with_pose_and_skin, CanvasPose, Quality};
use grok_ball::skin;
use grok_ball::types::BallExpression;
use lvgl::sys as lvgl_sys;
use lvgl::Display;
use lvgl_ui::{App, Node, Theme, Ui};
use mosaico_s31::ball_widget::{BallControl, BallWidget, CANVAS_SIZE};
use mosaico_s31::boot_splash::{self, SPLASH_HEIGHT, SPLASH_WIDTH};
use mosaico_s31::entrance;

const DISPLAY_WIDTH: u32 = 480;
const DISPLAY_HEIGHT: u32 = 480;
const DISPLAY_BUFFER_PIXELS: usize = (DISPLAY_WIDTH as usize) * 4;
const BYTES_PER_PIXEL: usize = 3;

type Capture = Rc<RefCell<Vec<u8>>>;

struct PreviewApp {
    control: Rc<BallControl>,
}

impl App for PreviewApp {
    type Msg = ();

    fn update(&mut self, (): ()) {}

    fn view(&self, _theme: &Theme) -> Node<()> {
        Node::custom(BallWidget::new(Rc::clone(&self.control)))
    }
}

fn main() {
    let output = output_dir();
    prepare_output_dir(&output);
    let output = output.canonicalize().expect("canonical output path");

    write_splash_frames(&output);
    write_entrance_frames(&output);
    write_skin_frames(&output);

    println!("preview frames in {}", output.display());
}

/// The wordmark timeline: reveal, underline, fade.
fn write_splash_frames(output: &Path) {
    let mut buffer = [0u16; SPLASH_WIDTH * SPLASH_HEIGHT];
    let mut previous: Option<u64> = None;
    let underline_done_ms =
        (boot_splash::WORDMARK.len() as u64 - 1) * boot_splash::LETTER_STAGGER_MS
            + boot_splash::LETTER_REVEAL_MS
            + boot_splash::UNDERLINE_MS;
    for ms in [0u64, 120, 260, 480, 700, 900, 1_000, 1_050, 1_340] {
        let finished = boot_splash::render(&mut buffer, ms);
        let path = output.join(format!("splash-{ms:04}.ppm"));
        write_ppm_rgb565(&path, &buffer, SPLASH_WIDTH, SPLASH_HEIGHT);
        let lit = buffer.iter().filter(|&&pixel| pixel != 0).count();
        let hash = hash_u16(&buffer);
        println!(
            "splash {ms:04}ms: {lit} lit pixels, finished={finished}, hash=0x{hash:016x}, {path}",
            path = path.display()
        );
        // Frames before the underline completes must all differ; afterwards the
        // hold is intentionally static until the fade.
        if let Some(last) = previous {
            if ms <= underline_done_ms {
                assert_ne!(hash, last, "each reveal step must differ before the hold");
            }
        }
        previous = Some(hash);
    }
}

/// The entrance drop, composited into a 480x480 black frame.
fn write_entrance_frames(output: &Path) {
    let mut splash = [0u16; SPLASH_WIDTH * SPLASH_HEIGHT];
    // A settled Idle frame provides the face.
    let mut control = BallControl::new(BallExpression::Idle);
    Rc::get_mut(&mut control).expect("sole owner");
    let control = control;
    let mut canvas = [0u16; CANVAS_SIZE * CANVAS_SIZE];

    let frame = {
        // Use the engine directly: the widget path needs an LVGL display.
        let mut engine = grok_ball::engine::BallEngine::new();
        engine.set_expression(BallExpression::Idle);
        engine.advance(500)
    };

    let mut composite = vec![0u16; (DISPLAY_WIDTH * DISPLAY_HEIGHT) as usize];
    for ms in [0u64, 120, 240, 380, 440, 520, 620, 760, 830, 900] {
        let pose = entrance::pose(ms);
        let canvas_pose = CanvasPose {
            scale_x: pose.scale_x,
            scale_y: pose.scale_y,
            offset_x: 0.0,
            offset_y: 0.0,
            rotate_deg: 0.0,
        };
        render_sized_with_pose_and_skin(
            &frame,
            &mut canvas,
            CANVAS_SIZE,
            Quality::TwoByOne,
            canvas_pose,
            &skin::BLOB,
        );

        composite.fill(0);
        blit(
            &mut composite,
            DISPLAY_WIDTH as usize,
            &canvas,
            CANVAS_SIZE,
            ((DISPLAY_WIDTH as i32 - CANVAS_SIZE as i32) / 2),
            (DISPLAY_HEIGHT as i32 - CANVAS_SIZE as i32) / 2 + pose.offset_y,
        );

        let path = output.join(format!("entrance-{ms:04}.ppm"));
        write_ppm_rgb565(&path, &composite, DISPLAY_WIDTH as usize, DISPLAY_HEIGHT as usize);
        let lit = composite.iter().filter(|&&pixel| pixel != 0).count();
        println!(
            "entrance {ms:04}ms: offset_y={} scale=({:.2},{:.2}) {lit} lit px, {path}",
            pose.offset_y,
            pose.scale_x,
            pose.scale_y,
            path = path.display()
        );
    }

    let _ = &mut splash;
    let _ = control;
}

/// The cat and fox skins next to the plain blob.
fn write_skin_frames(output: &Path) {
    let frame = {
        let mut engine = grok_ball::engine::BallEngine::new();
        engine.set_expression(BallExpression::Happy);
        engine.advance(500)
    };
    let mut canvas = [0u16; CANVAS_SIZE * CANVAS_SIZE];
    for (name, skin) in [
        ("blob", &skin::BLOB),
        ("cat", &skin::CAT),
        ("fox", &skin::FOX),
    ] {
        render_sized_with_pose_and_skin(
            &frame,
            &mut canvas,
            CANVAS_SIZE,
            Quality::TwoByOne,
            CanvasPose::IDENTITY,
            skin,
        );
        let path = output.join(format!("skin-{name}.ppm"));
        write_ppm_rgb565(&path, &canvas, CANVAS_SIZE, CANVAS_SIZE);
        let lit = canvas.iter().filter(|&&pixel| pixel != 0).count();
        println!("skin {name}: {lit} lit px, {path}", path = path.display());
    }
}

fn blit(dst: &mut [u16], dst_width: usize, src: &[u16], src_size: usize, x: i32, y: i32) {
    for sy in 0..src_size {
        let dy = y + sy as i32;
        if dy < 0 || dy >= DISPLAY_HEIGHT as i32 {
            continue;
        }
        for sx in 0..src_size {
            let dx = x + sx as i32;
            if dx < 0 || dx >= dst_width as i32 {
                continue;
            }
            dst[dy as usize * dst_width + dx as usize] = src[sy * src_size + sx];
        }
    }
}

fn write_ppm_rgb565(path: &Path, pixels: &[u16], width: usize, height: usize) {
    let mut file = fs::File::create(path).expect("create PPM");
    write!(file, "P6\n{width} {height}\n255\n").expect("PPM header");
    let mut row = Vec::with_capacity(width * BYTES_PER_PIXEL);
    for chunk in pixels.chunks(width) {
        row.clear();
        for &pixel in chunk {
            let r = ((pixel >> 11) & 0x1f) as u8;
            let g = ((pixel >> 5) & 0x3f) as u8;
            let b = (pixel & 0x1f) as u8;
            row.push((r << 3) | (r >> 2));
            row.push((g << 2) | (g >> 4));
            row.push((b << 3) | (b >> 2));
        }
        file.write_all(&row).expect("PPM row");
    }
}

fn hash_u16(pixels: &[u16]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &pixel in pixels {
        for byte in pixel.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("splash-preview")
}

fn prepare_output_dir(output: &Path) {
    if output.exists() {
        fs::remove_dir_all(output).expect("clear previous preview output");
    }
    fs::create_dir_all(output).expect("create preview output directory");
}

// Keep LVGL linked for the shared widget types used by the ball preview; these
// items are otherwise unused here.
#[allow(dead_code)]
fn _unused(_: *mut lvgl_sys::lv_obj_t, _: Display, _: Ui<()>, _: Capture, _: ptr::NonNull<u8>) {}
