//! Bounded, read-only shadow fingerprints for the Album popup investigation.
//!
//! Owned by the sole native UI task. No allocation, LVGL calls, synchronization,
//! or logging occurs here. The caller supplies a stable shadow without awaiting.
//! This module is absent from ordinary firmware builds.

// #region debug-point popup-shadow-diagnostics
use core::fmt;

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 480;
pub const FRAME_LIMIT: u8 = 8;
pub const SETTLE_US: u64 = 450_000;
const LINE_INTERVAL_US: u64 = 20_000;
pub const STARTUP_MARKER: &str =
    "popup-crc marker=n5-shadow-20261025-r1 max=8 settle_ms=450 cols=480 rgb565=le crc=zlib";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Open,
    Close,
    Delete,
}
impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Close => "close",
            Self::Delete => "delete",
        })
    }
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < table.len() {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xedb8_8320 } else { 0 };
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}
const CRC_TABLE: [u32; 256] = crc_table();

fn byte(crc: u32, value: u8) -> u32 {
    (crc >> 8) ^ CRC_TABLE[((crc ^ u32::from(value)) & 255) as usize]
}

/// Whole-frame row-major RGB565 LE CRC and 480 column CRCs, each top to bottom.
/// Standard reflected IEEE CRC32: polynomial edb88320, initial/final xor ffffffff.
pub fn fingerprint(pixels: &[u16], columns: &mut [u32; WIDTH]) -> Option<u32> {
    if pixels.len() != WIDTH * HEIGHT {
        return None;
    }
    columns.fill(u32::MAX);
    let mut frame = u32::MAX;
    for row in pixels.chunks_exact(WIDTH) {
        for (column, &pixel) in columns.iter_mut().zip(row) {
            let [lo, hi] = pixel.to_le_bytes();
            frame = byte(byte(frame, lo), hi);
            *column = byte(byte(*column, lo), hi);
        }
    }
    for column in columns {
        *column = !*column;
    }
    Some(!frame)
}

#[derive(Clone, Copy)]
struct Pending {
    seq: u32,
    cause: Cause,
    observed_at: Option<u64>,
    published: bool,
    handled: bool,
}
#[derive(Clone, Copy)]
struct Sample {
    seq: u32,
    cause: Cause,
    at: u64,
    micros: u64,
    frame: u32,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Begin {
        seq: u32,
        cause: Cause,
        at: u64,
        micros: u64,
        frame: u32,
    },
    Columns {
        seq: u32,
        x: usize,
        values: [u32; 4],
    },
    End {
        seq: u32,
        frame: u32,
    },
    Superseded {
        first: u32,
        last: u32,
        count: u32,
        by: u32,
    },
    Cancelled {
        pending: u32,
        stream: u32,
    },
    Limit,
}
impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Begin {
                seq,
                cause,
                at,
                micros,
                frame,
            } => write!(
                f,
                "popup-crc begin q={seq} why={cause} fc={frame:08x} at={at} us={micros} n=480"
            ),
            Self::Columns {
                seq,
                x,
                values: [a, b, c, d],
            } => write!(
                f,
                "popup-crc cols q={seq} x={x} v={a:08x},{b:08x},{c:08x},{d:08x}"
            ),
            Self::End { seq, frame } => write!(f, "popup-crc end q={seq} fc={frame:08x}"),
            Self::Superseded {
                first,
                last,
                count,
                by,
            } => write!(
                f,
                "popup-crc skip first={first} last={last} n={count} by={by} why=new-action"
            ),
            Self::Cancelled { pending, stream } => write!(
                f,
                "popup-crc cancel pending={pending} stream={stream} why=leave"
            ),
            Self::Limit => f.write_str("popup-crc limit max=8 reset=reboot"),
        }
    }
}

pub struct Diagnostics {
    next_seq: u32,
    pending: Option<Pending>,
    sample: Option<Sample>,
    columns: [u32; WIDTH],
    captured: u8,
    last_line_at: Option<u64>,
    superseded: Option<(u32, u32, u32)>,
    cancelled: Option<(u32, u32)>,
    limit_reported: bool,
}
impl Diagnostics {
    pub const fn new() -> Self {
        Self {
            next_seq: 0,
            pending: None,
            sample: None,
            columns: [0; WIDTH],
            captured: 0,
            last_line_at: None,
            superseded: None,
            cancelled: None,
            limit_reported: false,
        }
    }
    /// Called only after a successful popup create/close/delete operation.
    pub fn action(&mut self, cause: Cause) {
        if self.captured == FRAME_LIMIT {
            return;
        }
        self.next_seq = self.next_seq.saturating_add(1);
        if let Some(old) = self.pending {
            self.superseded = Some(match self.superseded {
                Some((first, _, count)) => (first, old.seq, count.saturating_add(1)),
                None => (old.seq, old.seq, 1),
            });
        }
        self.pending = Some(Pending {
            seq: self.next_seq,
            cause,
            observed_at: None,
            published: false,
            handled: false,
        });
    }
    /// Publication is necessary but not sufficient: a later handler must run.
    pub fn frame_end(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.published = true;
            pending.handled = false;
        }
    }
    pub fn handler_finished(&mut self) {
        if let Some(pending) = &mut self.pending {
            if pending.published {
                pending.handled = true;
            }
        }
    }
    /// Keep the boot-wide sample limit when a script releases or leaves its page.
    pub fn cancel(&mut self) {
        let pending = self.pending.take().map_or(0, |p| p.seq);
        let stream = self.sample.take().map_or(0, |s| s.seq);
        if pending != 0 || stream != 0 {
            self.cancelled = Some((pending, stream));
        }
    }
    /// `stable` is supplied only after the last damage was submitted and its
    /// plane lease released. The caller must not await or run LVGL during this
    /// synchronous read. Quiet time starts at the first UI observation of the
    /// action, conservatively later than the action's actual RPC timestamp.
    pub fn sample_if_ready(&mut self, now: u64, active: bool, stable: Option<&[u16]>) -> bool {
        if !active {
            self.cancel();
            return false;
        }
        let Some(pending) = &mut self.pending else {
            return false;
        };
        let observed = *pending.observed_at.get_or_insert(now);
        if !pending.published
            || !pending.handled
            || now.saturating_sub(observed) < SETTLE_US
            || self.sample.is_some()
            || self.captured == FRAME_LIMIT
        {
            return false;
        }
        let Some(pixels) = stable else { return false };
        let Some(frame) = fingerprint(pixels, &mut self.columns) else {
            return false;
        };
        self.sample = Some(Sample {
            seq: pending.seq,
            cause: pending.cause,
            at: now,
            micros: 0,
            frame,
            cursor: 0,
        });
        self.pending = None;
        self.captured += 1;
        true
    }
    pub fn set_sample_micros(&mut self, micros: u64) {
        if let Some(sample) = &mut self.sample {
            sample.micros = micros;
        }
    }
    /// At most one short line every20ms, from the existing UI loop.
    pub fn next_line(&mut self, now: u64) -> Option<Line> {
        if self
            .last_line_at
            .is_some_and(|last| now.saturating_sub(last) < LINE_INTERVAL_US)
        {
            return None;
        }
        let line = if let Some((first, last, count)) = self.superseded.take() {
            Line::Superseded {
                first,
                last,
                count,
                by: self.next_seq,
            }
        } else if let Some((pending, stream)) = self.cancelled.take() {
            Line::Cancelled { pending, stream }
        } else if let Some(sample) = &mut self.sample {
            let line = match sample.cursor {
                0 => Line::Begin {
                    seq: sample.seq,
                    cause: sample.cause,
                    at: sample.at,
                    micros: sample.micros,
                    frame: sample.frame,
                },
                1..=120 => {
                    let x = (sample.cursor - 1) * 4;
                    Line::Columns {
                        seq: sample.seq,
                        x,
                        values: self.columns[x..x + 4].try_into().unwrap(),
                    }
                }
                _ => Line::End {
                    seq: sample.seq,
                    frame: sample.frame,
                },
            };
            sample.cursor += 1;
            if sample.cursor == 122 {
                self.sample = None;
            }
            line
        } else if self.captured == FRAME_LIMIT && !self.limit_reported {
            self.limit_reported = true;
            Line::Limit
        } else {
            return None;
        };
        self.last_line_at = Some(now);
        Some(line)
    }
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self::new()
    }
}
// #endregion debug-point popup-shadow-diagnostics

#[cfg(test)]
mod tests {
    use super::*;
    use std::{format, vec, vec::Vec};

    fn pixels() -> Vec<u16> {
        vec![0x1234; WIDTH * HEIGHT]
    }
    fn ready(d: &mut Diagnostics, at: u64, pixels: &[u16]) {
        d.action(Cause::Open);
        assert!(!d.sample_if_ready(at, true, Some(pixels)));
        d.frame_end();
        d.handler_finished();
        assert!(d.sample_if_ready(at + SETTLE_US, true, Some(pixels)));
    }
    fn drain(d: &mut Diagnostics, start: u64) -> Vec<Line> {
        (0..125)
            .filter_map(|i| d.next_line(start + i * LINE_INTERVAL_US))
            .collect()
    }
    #[test]
    fn waits_for_publication_later_handler_quiet_and_stable_shadow() {
        let pixels = pixels();
        let mut d = Diagnostics::new();
        d.action(Cause::Open);
        d.handler_finished(); // Handler before frame_end cannot satisfy the gate.
        assert!(!d.sample_if_ready(10, true, Some(&pixels)));
        d.frame_end();
        assert!(!d.sample_if_ready(10 + SETTLE_US, true, Some(&pixels)));
        d.handler_finished();
        assert!(!d.sample_if_ready(10 + SETTLE_US - 1, true, Some(&pixels)));
        d.frame_end(); // A newer publication also needs a subsequent handler.
        assert!(!d.sample_if_ready(10 + SETTLE_US, true, Some(&pixels)));
        d.handler_finished();
        assert!(!d.sample_if_ready(10 + SETTLE_US, true, None));
        assert!(d.sample_if_ready(10 + SETTLE_US, true, Some(&pixels)));
        assert!(!d.sample_if_ready(10 + SETTLE_US + 1, true, Some(&pixels)));
    }
    #[test]
    fn emits_one_complete_sample_paced_and_without_overwriting_its_columns() {
        let pixels = pixels();
        let mut d = Diagnostics::new();
        ready(&mut d, 0, &pixels);
        d.set_sample_micros(123);
        assert!(matches!(
            d.next_line(SETTLE_US),
            Some(Line::Begin {
                seq: 1,
                micros: 123,
                ..
            })
        ));
        assert!(d.next_line(SETTLE_US + LINE_INTERVAL_US - 1).is_none());
        d.action(Cause::Close);
        d.frame_end();
        d.handler_finished();
        assert!(!d.sample_if_ready(SETTLE_US + 1, true, Some(&pixels)));
        assert!(!d.sample_if_ready(SETTLE_US * 2 + 1, true, Some(&pixels)));
        let lines = drain(&mut d, SETTLE_US + LINE_INTERVAL_US);
        assert_eq!(lines.len(), 121);
        for (i, line) in lines[..120].iter().enumerate() {
            assert!(matches!(line, Line::Columns { seq: 1, x, .. } if *x == i * 4));
        }
        assert!(matches!(lines.last(), Some(Line::End { seq: 1, .. })));
        assert!(d.sample_if_ready(4_000_000, true, Some(&pixels)));
    }
    #[test]
    fn overrides_and_page_exit_are_explicit_and_limits_survive_release() {
        let pixels = pixels();
        let mut d = Diagnostics::new();
        d.action(Cause::Open);
        d.action(Cause::Close);
        d.action(Cause::Delete);
        assert_eq!(
            d.next_line(0),
            Some(Line::Superseded {
                first: 1,
                last: 2,
                count: 2,
                by: 3
            })
        );
        assert!(!d.sample_if_ready(100, false, Some(&pixels)));
        assert_eq!(
            d.next_line(LINE_INTERVAL_US),
            Some(Line::Cancelled {
                pending: 3,
                stream: 0
            })
        );
        for n in 0..FRAME_LIMIT {
            let at = 10_000_000 * (u64::from(n) + 1);
            ready(&mut d, at, &pixels);
            let lines = drain(&mut d, at + SETTLE_US);
            assert_eq!(
                lines
                    .iter()
                    .filter(|l| matches!(l, Line::Begin { .. }))
                    .count(),
                1
            );
            assert_eq!(
                lines.iter().filter(|l| matches!(l, Line::Limit)).count(),
                usize::from(n == FRAME_LIMIT - 1)
            );
            d.cancel();
        }
        d.action(Cause::Open);
        d.frame_end();
        d.handler_finished();
        assert!(!d.sample_if_ready(100_000_000, true, Some(&pixels)));
        assert!(d.next_line(100_000_000).is_none());
    }
    #[test]
    fn leaving_cancels_an_incomplete_stream_and_every_line_fits_console() {
        let pixels = pixels();
        let mut d = Diagnostics::new();
        ready(&mut d, 0, &pixels);
        d.next_line(SETTLE_US);
        d.action(Cause::Close);
        d.cancel();
        assert_eq!(
            d.next_line(SETTLE_US + LINE_INTERVAL_US),
            Some(Line::Cancelled {
                pending: 2,
                stream: 1
            })
        );
        assert!(d.next_line(SETTLE_US + 2 * LINE_INTERVAL_US).is_none());
        let lines = [
            Line::Begin {
                seq: u32::MAX,
                cause: Cause::Delete,
                at: u64::MAX,
                micros: u64::MAX,
                frame: u32::MAX,
            },
            Line::Columns {
                seq: u32::MAX,
                x: 476,
                values: [u32::MAX; 4],
            },
            Line::End {
                seq: u32::MAX,
                frame: u32::MAX,
            },
            Line::Superseded {
                first: u32::MAX,
                last: u32::MAX,
                count: u32::MAX,
                by: u32::MAX,
            },
            Line::Cancelled {
                pending: u32::MAX,
                stream: u32::MAX,
            },
            Line::Limit,
        ];
        for line in lines {
            assert!(format!("[INFO] {line}\r\n").len() <= 150);
        }
        assert!(format!("[INFO] {STARTUP_MARKER}\r\n").len() <= 150);
    }
    #[test]
    fn invalid_frame_does_not_modify_column_cache() {
        let mut columns = [123; WIDTH];
        assert_eq!(fingerprint(&[0; 3], &mut columns), None);
        assert_eq!(columns, [123; WIDTH]);
    }
}
