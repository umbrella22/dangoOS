//! Render the six AI states with the G0.3 rasterizer and write PPM files.
//!
//! Run from the workspace root:
//!
//! ```text
//! cargo run -p grok-ball --example states --offline
//! ```
//!
//! The PPM (P6) files are written to `target/grok-ball-states/`, one per
//! state, and the path of each file is printed on its own line.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use grok_ball::raster::{render, CANVAS_PIXELS, CANVAS_SIZE};
use grok_ball::{BallEngine, BallExpression};

/// The six AI states from RFC 0003 section 2.3, in display order.
const STATES: [(BallExpression, &str); 6] = [
    (BallExpression::Idle, "idle"),
    (BallExpression::Listening, "listening"),
    (BallExpression::Thinking, "thinking"),
    (BallExpression::Speaking, "speaking"),
    (BallExpression::Happy, "happy"),
    (BallExpression::Error, "error"),
];

fn write_ppm(path: &Path, pixels: &[u16; CANVAS_PIXELS]) -> std::io::Result<()> {
    let mut writer = BufWriter::new(fs::File::create(path)?);
    write!(writer, "P6\n{} {}\n255\n", CANVAS_SIZE, CANVAS_SIZE)?;

    let mut row = Vec::with_capacity(CANVAS_SIZE * 3);
    for &value in pixels {
        let r = ((value >> 11) & 0x1f) as u8;
        let g = ((value >> 5) & 0x3f) as u8;
        let b = (value & 0x1f) as u8;
        // Expand RGB565 to RGB888 with bit replication.
        row.push((r << 3) | (r >> 2));
        row.push((g << 2) | (g >> 4));
        row.push((b << 3) | (b >> 2));
    }
    writer.write_all(&row)?;
    writer.flush()
}

fn main() {
    let output_dir = PathBuf::from("target/grok-ball-states");
    fs::create_dir_all(&output_dir).expect("create target/grok-ball-states");

    for (expression, name) in STATES {
        let mut engine = BallEngine::new();
        engine.set_expression(expression);
        // Let the blink spring, gaze drift and breathing settle into a
        // representative pose instead of the first static frame.
        let frame = engine.advance(800);

        let mut pixels = [0u16; CANVAS_PIXELS];
        let dirty = render(&frame, &mut pixels);
        assert!(!dirty.is_empty(), "rasterizer returned an empty dirty rect");

        let path = output_dir.join(format!("{name}.ppm"));
        write_ppm(&path, &pixels).unwrap_or_else(|error| {
            panic!("write {}: {error}", path.display());
        });
        println!("{}", path.display());
    }
}
