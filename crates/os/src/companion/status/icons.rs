//! Small coverage masks generated at compile time from round, 2.25 px strokes.
//! Each byte stores the Wi-Fi segment (high bits) and coverage 0..16 (low bits).

pub const WIFI_WIDTH: usize = 30;
pub const WIFI_HEIGHT: usize = 26;
pub const BLUETOOTH_WIDTH: usize = 20;
pub const BLUETOOTH_HEIGHT: usize = 28;

const HALF_STROKE: i64 = 9; // Geometry uses eighth-pixel coordinates.

const fn circle(x: i64, y: i64, radius: i64) -> bool {
    x * x + y * y <= radius * radius
}

const fn arc(x: i64, y: i64, radius: i64) -> bool {
    let distance = x * x + y * y;
    let inner = radius - HALF_STROKE;
    let outer = radius + HALF_STROKE;
    let body = y <= 0 && x.abs() <= -y && distance >= inner * inner && distance <= outer * outer;
    // Round ends at +/-45 degrees; 181/256 approximates sqrt(1/2).
    let end = (radius * 181 + 128) / 256;
    body || circle(x - end, y + end, HALF_STROKE) || circle(x + end, y + end, HALF_STROKE)
}

const fn line(x: i64, y: i64, a: (i64, i64), b: (i64, i64)) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (px, py) = (x - a.0, y - a.1);
    let length = dx * dx + dy * dy;
    let projection = px * dx + py * dy;
    if projection < 0 {
        return circle(px, py, HALF_STROKE);
    }
    if projection > length {
        return circle(x - b.0, y - b.1, HALF_STROKE);
    }
    let cross = px * dy - py * dx;
    cross * cross <= HALF_STROKE * HALF_STROKE * length
}

const fn wifi() -> [u8; WIFI_WIDTH * WIFI_HEIGHT] {
    let mut mask = [0; WIFI_WIDTH * WIFI_HEIGHT];
    let mut pixel = 0;
    while pixel < mask.len() {
        let mut segment = 0;
        while segment < 4 {
            let mut coverage = 0;
            let mut sample = 0;
            while sample < 16 {
                let x = (pixel % WIFI_WIDTH) as i64 * 8 + (sample % 4) * 2 + 1 - 15 * 8;
                let y = (pixel / WIFI_WIDTH) as i64 * 8 + (sample / 4) * 2 + 1 - 22 * 8;
                let covered = if segment == 0 {
                    circle(x, y, 14)
                } else {
                    arc(x, y, (segment as i64 * 6 + 1) * 8)
                };
                if covered {
                    coverage += 1;
                }
                sample += 1;
            }
            if coverage > 0 {
                mask[pixel] = ((segment + 1) as u8) << 5 | coverage;
                break;
            }
            segment += 1;
        }
        pixel += 1;
    }
    mask
}

const fn bluetooth() -> [u8; BLUETOOTH_WIDTH * BLUETOOTH_HEIGHT] {
    let mut mask = [0; BLUETOOTH_WIDTH * BLUETOOTH_HEIGHT];
    let mut pixel = 0;
    while pixel < mask.len() {
        let mut coverage = 0;
        let mut sample = 0;
        while sample < 16 {
            let x = (pixel % BLUETOOTH_WIDTH) as i64 * 8 + (sample % 4) * 2 + 1;
            let y = (pixel / BLUETOOTH_WIDTH) as i64 * 8 + (sample / 4) * 2 + 1;
            // Standard Bluetooth bind rune, centred at (10, 14).
            if line(x, y, (80, 16), (80, 208))
                || line(x, y, (80, 16), (136, 64))
                || line(x, y, (136, 64), (24, 160))
                || line(x, y, (24, 64), (136, 160))
                || line(x, y, (136, 160), (80, 208))
            {
                coverage += 1;
            }
            sample += 1;
        }
        mask[pixel] = 1 << 5 | coverage;
        pixel += 1;
    }
    mask
}

pub static WIFI: [u8; WIFI_WIDTH * WIFI_HEIGHT] = wifi();
pub static BLUETOOTH: [u8; BLUETOOTH_WIDTH * BLUETOOTH_HEIGHT] = bluetooth();
