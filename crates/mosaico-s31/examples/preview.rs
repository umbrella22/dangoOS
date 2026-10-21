//! Headless host preview for [`mosaico_s31::BallWidget`].
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p mosaico-s31 --example preview --offline
//! ```
//!
//! The example registers a 480x480 RGB888 memory display (no SDL), mounts one
//! `Node::custom(BallWidget)`, and cycles through all 14 `BallExpression`
//! values. Each state runs for 125 ticks of 16 ms (2 seconds), then the display
//! buffer is forced out and written as a PPM (P6) image under
//! `target/ball-widget-preview/`.
//!
//! The widget is controlled only through the shared `BallControl` handle after
//! the initial mount, which proves that a later `Node::custom` descriptor is
//! not needed to change the expression.

use core::ptr;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use grok_ball::types::BallExpression;
use lvgl::sys as lvgl_sys;
use lvgl::Display;
use lvgl_ui::{App, Node, Theme, Ui};
use mosaico_s31::ball_widget::CANVAS_SIZE;
use mosaico_s31::{BallControl, BallWidget};

const DISPLAY_WIDTH: u32 = 480;
const DISPLAY_HEIGHT: u32 = 480;

/// Flush buffer for four display rows. `Display::register` needs at least one
/// aligned row; four rows also reduce the number of flush calls.
const DISPLAY_BUFFER_PIXELS: usize = (DISPLAY_WIDTH as usize) * 4;

const FRAME_MS: u32 = 16;
const FRAMES_PER_STATE: u32 = 125;
const BYTES_PER_PIXEL: usize = 3;

type Capture = Rc<RefCell<Vec<u8>>>;

struct PreviewApp {
    control: Rc<BallControl>,
}

impl App for PreviewApp {
    type Msg = ();

    fn update(&mut self, (): ()) {}

    fn view(&self, _theme: &Theme) -> Node<()> {
        // Built once at mount. Later reconciles would discard this descriptor;
        // the mounted instance is driven through `self.control`.
        Node::custom(BallWidget::new(Rc::clone(&self.control)))
    }
}

fn main() {
    // SAFETY: this is a single-threaded process that owns LVGL for its whole
    // lifetime and never touches it from a callback thread.
    unsafe { lvgl::deinit() };
    lvgl::init();

    let pixels = Rc::new(RefCell::new(vec![
        0u8;
        DISPLAY_WIDTH as usize
            * DISPLAY_HEIGHT as usize
            * BYTES_PER_PIXEL
    ]));
    let display = register_capturing_display(Rc::clone(&pixels));
    let mut ui = Ui::new(&display, Theme::dark()).expect("ui creation");

    let control = BallControl::new(BallExpression::Idle);
    let app = PreviewApp {
        control: Rc::clone(&control),
    };
    ui.mount(&app).expect("initial mount");
    assert_eq!(
        control.create_count(),
        1,
        "mount creates exactly one widget"
    );
    assert_eq!(control.delete_count(), 0);

    let canvas = mounted_canvas();
    verify_canvas(canvas);
    let (canvas_x, canvas_y) = canvas_position(canvas);
    println!(
        "mounted canvas: {size}x{size} at ({canvas_x}, {canvas_y}) inside {DISPLAY_WIDTH}x{DISPLAY_HEIGHT}",
        size = CANVAS_SIZE,
    );

    let output = output_dir();
    prepare_output_dir(&output);
    let output = output
        .canonicalize()
        .expect("canonical preview output path");

    let mut paths = Vec::new();
    let mut hashes = Vec::new();
    for (index, expression) in BallExpression::ALL.iter().copied().enumerate() {
        control.set_expression(expression);

        for _ in 0..FRAMES_PER_STATE {
            lvgl::tick_inc(Duration::from_millis(FRAME_MS as u64));
            ui.tick(FRAME_MS).expect("widget tick");
            ui.timer_handler();
        }

        // Force a full-screen refresh so the capture contains every pixel,
        // not only the canvas region invalidated by the widget.
        refresh_full_screen();

        let snapshot = pixels.borrow().clone();
        let path = output.join(format!("{index:02}-{expression:?}.ppm"));
        write_ppm(&path, &snapshot);
        let (canvas_colors, unique_colors) = color_stats(&snapshot, canvas_x, canvas_y);
        let hash = fnv1a(&snapshot);
        println!(
            "state {index:02} {expression:?}: {path} ({} bytes, {unique_colors} colors, canvas region {canvas_colors} colors, hash 0x{hash:016x})",
            snapshot.len(),
            path = path.display(),
        );
        paths.push(path);
        hashes.push(hash);
    }

    assert_eq!(
        control.create_count(),
        1,
        "changing the expression through BallControl must not remount the widget"
    );
    assert_eq!(control.delete_count(), 0, "widget is still mounted");
    assert!(
        control.update_count() >= u64::from(FRAMES_PER_STATE) * BallExpression::ALL.len() as u64,
        "every tick must reach the widget"
    );
    assert!(
        control.rendered_frames() > 0,
        "the widget must rasterize at least one frame"
    );
    assert_eq!(
        control.invalidations(),
        control.rendered_frames(),
        "the widget must invalidate only after a rendered frame"
    );
    println!(
        "lifecycle: creates={} updates={} rendered_frames={} invalidations={} deletes={} last_dirty_rect={:?}",
        control.create_count(),
        control.update_count(),
        control.rendered_frames(),
        control.invalidations(),
        control.delete_count(),
        control.last_dirty_rect(),
    );

    let all_equal = hashes.windows(2).all(|pair| pair[0] == pair[1]);
    println!(
        "captures: {} files in {}; all hashes equal: {all_equal}",
        paths.len(),
        output.display()
    );

    ui.unmount();
    assert_eq!(control.delete_count(), 1, "unmount deletes exactly once");
    assert_eq!(ui.stats().mounted_nodes, 0, "mounted tree is empty");
    // SAFETY: the screen is live and the UI root was just deleted; the count
    // is a plain field read.
    let screen_children = unsafe { lvgl_sys::lv_obj_get_child_count(active_screen()) };
    assert_eq!(screen_children, 0, "native screen has no leftover children");
    println!(
        "unmounted: native screen children={screen_children}; lifecycle creates={} updates={} rendered_frames={} invalidations={} deletes={}",
        control.create_count(),
        control.update_count(),
        control.rendered_frames(),
        control.invalidations(),
        control.delete_count(),
    );

    drop(ui);
    // SAFETY: the UI is unmounted and no object, callback, render or flush for
    // this display remains.
    unsafe { display.delete() };
    // SAFETY: the display was deleted and no LVGL object or handle is used
    // after this point in the process.
    unsafe { lvgl::deinit() };
}

fn register_capturing_display(pixels: Capture) -> Display {
    Display::register::<_, { DISPLAY_BUFFER_PIXELS }>(
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        move |refresh| {
            let width = (refresh.area.x2 - refresh.area.x1 + 1) as usize;
            let height = (refresh.area.y2 - refresh.area.y1 + 1) as usize;
            let stride = DISPLAY_WIDTH as usize * BYTES_PER_PIXEL;
            let mut pixels = pixels.borrow_mut();
            for y in 0..height {
                for x in 0..width {
                    let color = refresh.colors[y * width + x];
                    let index = (refresh.area.y1 as usize + y) * stride
                        + (refresh.area.x1 as usize + x) * BYTES_PER_PIXEL;
                    pixels[index] = color.r();
                    pixels[index + 1] = color.g();
                    pixels[index + 2] = color.b();
                }
            }
        },
    )
    .expect("480x480 memory display registration")
}

fn active_screen() -> *mut lvgl_sys::lv_obj_t {
    // SAFETY: a display is registered for the lifetime of the call.
    unsafe { lvgl_sys::lv_screen_active() }
}

/// The mounted custom canvas: screen -> Ui root -> app root (the custom node).
fn mounted_canvas() -> *mut lvgl_sys::lv_obj_t {
    let screen = active_screen();
    assert!(!screen.is_null(), "active screen");
    // SAFETY: `screen` is live and child 0 is the `Ui` root container.
    let root = unsafe { lvgl_sys::lv_obj_get_child(screen, 0) };
    assert!(!root.is_null(), "Ui root container");
    // SAFETY: the app view returns a single custom node, so child 0 of the Ui
    // root is the canvas created by `BallWidget::create`.
    let canvas = unsafe { lvgl_sys::lv_obj_get_child(root, 0) };
    assert!(!canvas.is_null(), "ball canvas");
    canvas
}

fn verify_canvas(canvas: *mut lvgl_sys::lv_obj_t) {
    // SAFETY: `canvas` is the live mounted object. Layout is normally run by
    // `lv_timer_handler`; run it once here so object coordinates are final
    // before we assert geometry.
    unsafe { lvgl_sys::lv_obj_update_layout(canvas) };
    // SAFETY: `canvas` is the live mounted object.
    let (width, height) = unsafe {
        (
            lvgl_sys::lv_obj_get_width(canvas),
            lvgl_sys::lv_obj_get_height(canvas),
        )
    };
    assert_eq!(width, CANVAS_SIZE as i32, "canvas width");
    assert_eq!(height, CANVAS_SIZE as i32, "canvas height");
}

fn canvas_position(canvas: *mut lvgl_sys::lv_obj_t) -> (i32, i32) {
    // SAFETY: `canvas` is the live mounted object.
    let (x, y) = unsafe {
        (
            lvgl_sys::lv_obj_get_x(canvas),
            lvgl_sys::lv_obj_get_y(canvas),
        )
    };
    let expected = (DISPLAY_WIDTH as i32 - CANVAS_SIZE as i32) / 2;
    assert_eq!(x, expected, "canvas horizontally centered");
    assert_eq!(y, expected, "canvas vertically centered");
    (x, y)
}

fn refresh_full_screen() {
    // SAFETY: called on the UI thread outside `lv_timer_handler`; the display
    // and screen are live.
    unsafe {
        let screen = active_screen();
        lvgl_sys::lv_obj_invalidate(screen);
        lvgl_sys::lv_refr_now(ptr::null_mut());
    }
}

fn output_dir() -> PathBuf {
    // `cargo run` from the workspace root writes to <workspace>/target/...;
    // anchoring on CARGO_MANIFEST_DIR keeps that true from any working dir.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("ball-widget-preview")
}

fn prepare_output_dir(output: &Path) {
    if output.exists() {
        fs::remove_dir_all(output).expect("clear previous preview output");
    }
    fs::create_dir_all(output).expect("create preview output directory");
}

fn write_ppm(path: &Path, pixels: &[u8]) {
    let mut file = fs::File::create(path).expect("create PPM");
    write!(file, "P6\n{} {}\n255\n", DISPLAY_WIDTH, DISPLAY_HEIGHT).expect("write PPM header");
    file.write_all(pixels).expect("write PPM pixels");
}

/// Returns (unique colors inside the canvas rect, unique colors in the whole
/// display capture). A placeholder rasterizer that fills zeroes produces one
/// color in the canvas region.
fn color_stats(pixels: &[u8], canvas_x: i32, canvas_y: i32) -> (usize, usize) {
    let mut all = BTreeSet::new();
    let mut canvas = BTreeSet::new();
    let width = DISPLAY_WIDTH as usize;
    for y in 0..DISPLAY_HEIGHT as usize {
        for x in 0..width {
            let index = (y * width + x) * BYTES_PER_PIXEL;
            let color = (pixels[index], pixels[index + 1], pixels[index + 2]);
            all.insert(color);
            let cx = x as i32;
            let cy = y as i32;
            if cx >= canvas_x
                && cy >= canvas_y
                && cx < canvas_x + CANVAS_SIZE as i32
                && cy < canvas_y + CANVAS_SIZE as i32
            {
                canvas.insert(color);
            }
        }
    }
    (canvas.len(), all.len())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
