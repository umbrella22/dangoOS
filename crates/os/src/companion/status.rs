//! Compact Home link icons composited into the existing RGB565 character canvas.
//! The panel plane and LVGL share the same pixels and service snapshot.
use crate::companion::render::{H, W};
use crate::services::radio::{BluetoothState, Snapshot, WifiLink, WifiState};

mod icons;

/// A centred pair inside the round panel's top safe area.
pub const AREA: (usize, usize, usize, usize) = ((W - 80) / 2, 4, 80, 34);
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
        out[(y + index / width) * W + x + index % width] = r << 11 | g << 5 | b;
    }
}

/// Draw into a leased frame. No heap allocation, extra canvas or LVGL object.
pub fn render(out: &mut [u16], status: Snapshot) {
    assert_eq!(out.len(), W * H);
    let (x, y, width, height) = AREA;
    for row in y..y + height {
        out[row * W + x..row * W + x + width].fill(0);
    }

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
    draw(out, &icons::WIFI, icons::WIFI_WIDTH, W / 2 - 31, 8, &wifi);

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
        W / 2 + 12,
        8,
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
