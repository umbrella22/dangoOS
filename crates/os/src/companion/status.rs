//! Home link indicators composited into the existing RGB565 character canvas.
//! The panel plane uploads this canvas directly, without drawing native labels.
use core::fmt::{self, Write};

use crate::companion::render::{H, W};
use crate::services::display::bitmap_font;
use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};

/// Canvas coordinates inside the round panel's top safe area.
pub const AREA: (usize, usize, usize, usize) = (18, 4, 316, 34);
const MUTED: u16 = 0x7bef;
const DIM: u16 = 0x2945;
const CONNECTED: u16 = 0x8f16;
const BLUETOOTH: u16 = 0x6d9f;
const PENDING: u16 = 0xfda8;

/// Three Wi-Fi arcs and a dot; unknown signal has no illuminated arcs.
fn signal_arcs(rssi_dbm: Option<i32>) -> u8 {
    match rssi_dbm {
        Some(-55..) => 3,
        Some(-67..=-56) => 2,
        Some(-80..=-68) => 1,
        _ => 0,
    }
}

struct Text<'a> {
    out: &'a mut [u16],
    x: usize,
    end: usize,
    color: u16,
}
impl Write for Text<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for ch in text.chars() {
            if self.x + 10 > self.end {
                break;
            }
            for (row, bits) in bitmap_font::glyph(ch).iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                self.out[(14 + row * 2 + dy) * W + self.x + col * 2 + dx] =
                                    self.color;
                            }
                        }
                    }
                }
            }
            self.x += 12;
        }
        Ok(())
    }
}

/// Draw into a leased frame. No heap allocation, extra canvas or LVGL object.
pub fn render(out: &mut [u16], status: Snapshot) {
    assert_eq!(out.len(), W * H);
    let (x, y, width, height) = AREA;
    for row in y..y + height {
        out[row * W + x..row * W + x + width].fill(0);
    }

    let linked = matches!(status.wifi_link, WifiLink::Connected { .. });
    let wifi_color = if linked {
        CONNECTED
    } else {
        match status.wifi {
            WifiState::Off | WifiState::Idle => MUTED,
            _ => PENDING,
        }
    };
    let arcs = match status.wifi_link {
        WifiLink::Connected { rssi_dbm } => signal_arcs(rssi_dbm),
        WifiLink::Disconnected => 0,
    };
    for dy in -18i32..=0 {
        for dx in -18i32..=18 {
            let distance = dx * dx + dy * dy;
            if dx.abs() > -dy {
                continue;
            }
            for (arc, radius) in [7, 12, 17].into_iter().enumerate() {
                if distance >= (radius - 2) * (radius - 2) && distance <= radius * radius {
                    out[(30 + dy) as usize * W + (41 + dx) as usize] = if arc < usize::from(arcs) {
                        wifi_color
                    } else {
                        DIM
                    };
                }
            }
        }
    }
    for y in 28..31 {
        for x in 40..43 {
            out[y * W + x] = wifi_color;
        }
    }
    let mut text = Text {
        out,
        x: 67,
        end: 199,
        color: wifi_color,
    };
    match status.wifi_link {
        WifiLink::Connected {
            rssi_dbm: Some(rssi),
        } => {
            let _ = write!(text, "{rssi}dbm");
        }
        WifiLink::Connected { rssi_dbm: None } => {
            let _ = text.write_str("--dbm");
        }
        WifiLink::Disconnected => {
            let _ = text.write_str(match status.wifi {
                WifiState::Off => "off",
                WifiState::Idle => "idle",
                WifiState::Scanning => "scan",
                WifiState::Associating => "join",
                WifiState::Dhcp => "dhcp",
                WifiState::Reconnecting => "retry",
                WifiState::Fault => "error",
                WifiState::Connected => "--dbm",
            });
        }
    }
    for y in 11..31 {
        out[y * W + 205] = DIM;
    }

    let (label, color) = match status.bluetooth {
        BluetoothState::Off => ("off", MUTED),
        BluetoothState::Starting => ("start", PENDING),
        BluetoothState::Discoverable => ("ready", MUTED),
        BluetoothState::Connected => ("link", BLUETOOTH),
        BluetoothState::Streaming => ("play", BLUETOOTH),
        BluetoothState::Fault => ("error", PENDING),
    };
    // Bluetooth rune, centred optically alongside the 14-pixel text.
    for dy in 0i32..22 {
        for dx in 0i32..14 {
            let stem = (6..=7).contains(&dx);
            let upper = dy <= 11 && ((dx - 6) * 2 - dy).abs() <= 1;
            let lower = dy >= 10 && ((dx - 6) * 2 - (21 - dy)).abs() <= 1;
            let cross_a = (dx * 2 - dy).abs() <= 1;
            let cross_b = (dx * 2 - (21 - dy)).abs() <= 1;
            if stem || upper || lower || cross_a || cross_b {
                out[(10 + dy) as usize * W + (221 + dx) as usize] = color;
            }
        }
    }
    let _ = Text {
        out,
        x: 245,
        end: 332,
        color,
    }
    .write_str(label);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signal_arcs_distinguish_weak_links_and_unavailable_measurements() {
        for (dbm, arcs) in [
            (None, 0),
            (Some(-95), 0),
            (Some(-80), 1),
            (Some(-68), 1),
            (Some(-67), 2),
            (Some(-56), 2),
            (Some(-55), 3),
        ] {
            assert_eq!(signal_arcs(dbm), arcs);
        }
    }
    #[test]
    fn every_status_is_confined_to_the_top_band() {
        for wifi in [
            WifiState::Off,
            WifiState::Idle,
            WifiState::Scanning,
            WifiState::Associating,
            WifiState::Dhcp,
            WifiState::Connected,
            WifiState::Fault,
            WifiState::Reconnecting,
        ] {
            for bluetooth in [
                BluetoothState::Off,
                BluetoothState::Starting,
                BluetoothState::Discoverable,
                BluetoothState::Connected,
                BluetoothState::Streaming,
                BluetoothState::Fault,
            ] {
                for wifi_link in [
                    WifiLink::Disconnected,
                    WifiLink::Connected { rssi_dbm: None },
                    WifiLink::Connected {
                        rssi_dbm: Some(-55),
                    },
                ] {
                    let mut out = alloc::vec![0xffff; W * H];
                    render(
                        &mut out,
                        Snapshot {
                            wifi,
                            wifi_link,
                            bluetooth,
                            ip: None,
                        },
                    );
                    let (x, y, width, height) = AREA;
                    for (index, pixel) in out.iter().enumerate() {
                        if !(x..x + width).contains(&(index % W))
                            || !(y..y + height).contains(&(index / W))
                        {
                            assert_eq!(*pixel, 0xffff, "status overwrites character at {index}");
                        }
                    }
                }
            }
        }
    }
}
