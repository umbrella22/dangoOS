//! Compact Home link icons in a separate band above the character canvas.
//! Its pixels share the character's frame lease on both the panel and LVGL.
use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};

mod icons;

pub const WIDTH: usize = 80;
pub const HEIGHT: usize = 34;
pub const PIXELS: usize = WIDTH * HEIGHT;
/// Fixed panel position, independent of the character's motion.
pub const OFFSET: (i32, i32) = (((super::render::VIEW_W - WIDTH) / 2) as i32, 40);
const MUTED: u16 = 0x7bef;
const DIM: u16 = 0x2945;
const CONNECTED: u16 = 0x8f16;
const BLUETOOTH: u16 = 0x6d9f;
const PENDING: u16 = 0xfda8;
const FAULT: u16 = 0xfacb;

/// Three measured strength levels; zero arcs means no RSSI sample is available.
fn signal_arcs(rssi_dbm: Option<i32>) -> u8 {
    match rssi_dbm {
        Some(-55..) => 3,
        Some(-67..=-56) => 2,
        Some(_) => 1,
        None => 0,
    }
}

fn draw(out: &mut [u16], mask: &[u8], width: usize, x: usize, y: usize, colors: &[u16]) {
    for (index, &sample) in mask.iter().enumerate() {
        let alpha = u16::from(sample & 31);
        if alpha == 0 {
            continue;
        }
        let color = colors[usize::from(sample >> 5)];
        // Coverage over the cleared black band; full coverage is exactly RGB565.
        let r = (((color >> 11) & 31) * alpha + 8) / 16;
        let g = (((color >> 5) & 63) * alpha + 8) / 16;
        let b = ((color & 31) * alpha + 8) / 16;
        out[(y + index / width) * WIDTH + x + index % width] = r << 11 | g << 5 | b;
    }
}

/// Draw the complete band into its part of the leased frame, without allocating.
pub fn render(out: &mut [u16], status: Snapshot) {
    assert_eq!(out.len(), PIXELS);
    out.fill(0);

    let wifi_color = match status.wifi_link {
        WifiLink::Connected { .. } => CONNECTED,
        WifiLink::Disconnected => match status.wifi {
            WifiState::Off => DIM,
            WifiState::Idle => MUTED,
            WifiState::Fault => FAULT,
            _ => PENDING,
        },
    };
    let arcs = match status.wifi_link {
        WifiLink::Connected { rssi_dbm } => signal_arcs(rssi_dbm),
        WifiLink::Disconnected => 0,
    };
    let mut wifi = [DIM; 5];
    wifi[1] = wifi_color;
    for arc in 0..usize::from(arcs) {
        wifi[arc + 2] = CONNECTED;
    }
    draw(out, &icons::WIFI, icons::WIFI_WIDTH, 9, 4, &wifi);

    let bluetooth_color = match status.bluetooth {
        BluetoothState::Off => DIM,
        BluetoothState::Starting => PENDING,
        BluetoothState::Discoverable => MUTED,
        BluetoothState::Connected | BluetoothState::Streaming => BLUETOOTH,
        BluetoothState::Fault => FAULT,
    };
    draw(
        out,
        &icons::BLUETOOTH,
        icons::BLUETOOTH_WIDTH,
        52,
        4,
        &[0, bluetooth_color],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signal_arcs_distinguish_weak_links_and_unavailable_measurements() {
        for (dbm, arcs) in [
            (None, 0),
            (Some(-95), 1),
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
    fn every_status_replaces_the_band_without_touching_adjacent_frame_pixels() {
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
                    let mut out = alloc::vec![0xffff; PIXELS + 2];
                    render(
                        &mut out[1..=PIXELS],
                        Snapshot {
                            wifi,
                            wifi_link,
                            bluetooth,
                            ip: None,
                        },
                    );
                    assert_eq!(out[0], 0xffff);
                    assert_eq!(out[PIXELS + 1], 0xffff);
                    assert!(out[1..=PIXELS].iter().all(|&p| p != 0xffff));
                }
            }
        }
    }
}
