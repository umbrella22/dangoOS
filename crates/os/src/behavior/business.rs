//! Connection edges and playback state. Animation loop length never expires a
//! business expression. The latest connection edge is the resting business
//! state; playback masks it until stopped. Reactions and battery remain above it.
use super::{Expression, affect::{EventKind, EventSource, SensorEvent}};
use crate::{audio, radio};

#[derive(Default)]
pub struct State {
    network: Option<bool>,
    bluetooth: Option<bool>,
    connection: Option<(Expression, u64)>,
    music_since: Option<u64>,
}
impl State {
    pub fn observe(&mut self, at: u64, kind: EventKind) {
        let (last, connected, on, off) = match kind {
            EventKind::NetworkConnection(value) => (&mut self.network, value,
                Expression::NetworkConnected, Expression::NetworkDisconnected),
            EventKind::BluetoothConnection(value) => (&mut self.bluetooth, value,
                Expression::BluetoothConnected, Expression::BluetoothDisconnected),
            EventKind::MusicPlayback(playing) => {
                if playing { self.music_since.get_or_insert(at); }
                else { self.music_since = None; }
                return;
            }
            _ => return,
        };
        if *last != Some(connected) {
            *last = Some(connected);
            self.connection = Some((if connected { on } else { off }, at));
        }
    }
    pub fn expression(&self) -> Option<(Expression, u64)> {
        self.music_since.map(|at| (Expression::MusicPlaying, at)).or(self.connection)
    }
}

/// UI-owned observation of service snapshots, including while away from Home.
/// Off at boot is a baseline, not a fabricated disconnect. A scan does not
/// imply link loss and Connected -> Streaming does not replay a connection.
#[derive(Default)]
pub struct Monitor {
    network: bool,
    bluetooth: bool,
    music: bool,
}
impl Monitor {
    pub fn observe(&mut self, at_ms: u64, radio: radio::Snapshot, audio: audio::State)
        -> [Option<SensorEvent>; 3]
    {
        let network = if radio.wifi == radio::WifiState::Scanning { self.network }
            else { radio.wifi == radio::WifiState::Connected };
        let bluetooth = matches!(radio.bluetooth, radio::BluetoothState::Connected | radio::BluetoothState::Streaming);
        let music = radio.bluetooth == radio::BluetoothState::Streaming || audio == audio::State::Playing;
        let values = [
            (network != self.network).then_some(EventKind::NetworkConnection(network)),
            (bluetooth != self.bluetooth).then_some(EventKind::BluetoothConnection(bluetooth)),
            (music != self.music).then_some(EventKind::MusicPlayback(music)),
        ];
        self.network = network;
        self.bluetooth = bluetooth;
        self.music = music;
        values.map(|kind| kind.map(|kind| SensorEvent { at_ms, source: EventSource::System, kind }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Runtime, ui::fox_widget::FoxControl};
    #[test]
    fn native_and_lua_hold_connections_and_release_playback_without_a_loop_timeout() {
        for lua in [false, true] {
            let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
            if lua {
                runtime.enable_brain(super::super::brain::DEFAULT_SOURCE, 0,
                    &lua_engine::GLOBAL_ALLOCATOR, None).unwrap();
            }
            let mut send = |at, kind, expected| {
                runtime.on_sensor(SensorEvent { at_ms: at, source: EventSource::System, kind });
                runtime.tick(at);
                assert_eq!(runtime.expression(), expected, "lua={lua}, at={at}");
            };
            send(10, EventKind::NetworkConnection(true), Expression::NetworkConnected);
            send(8_010, EventKind::NetworkConnection(true), Expression::NetworkConnected);
            send(60_000, EventKind::BluetoothConnection(true), Expression::BluetoothConnected);
            send(60_010, EventKind::MusicPlayback(true), Expression::MusicPlaying);
            send(180_010, EventKind::NetworkConnection(false), Expression::MusicPlaying);
            send(180_020, EventKind::MusicPlayback(false), Expression::NetworkDisconnected);
            send(180_030, EventKind::BluetoothConnection(false), Expression::BluetoothDisconnected);
            send(180_040, EventKind::Activity(Expression::Error), Expression::Error);
            send(180_050, EventKind::MusicPlayback(true), Expression::Error);
            send(180_060, EventKind::Activity(Expression::Idle), Expression::MusicPlaying);
            assert_eq!(runtime.brain_active(), lua);
        }
    }
    #[test]
    fn service_monitor_emits_link_edges_and_playback_without_scan_or_boot_disconnects() {
        let mut m = Monitor::default();
        let mut s = radio::Snapshot::new();
        assert!(m.observe(0, s, audio::State::Ready).iter().all(Option::is_none));
        s.wifi = radio::WifiState::Connected;
        assert_eq!(m.observe(1, s, audio::State::Ready)[0].unwrap().kind, EventKind::NetworkConnection(true));
        s.wifi = radio::WifiState::Scanning;
        assert!(m.observe(2, s, audio::State::Ready).iter().all(Option::is_none));
        s.wifi = radio::WifiState::Reconnecting;
        assert_eq!(m.observe(3, s, audio::State::Ready)[0].unwrap().kind, EventKind::NetworkConnection(false));
        s.bluetooth = radio::BluetoothState::Connected;
        assert!(m.observe(4, s, audio::State::Ready)[1].is_some());
        s.bluetooth = radio::BluetoothState::Streaming;
        let e = m.observe(5, s, audio::State::Playing);
        assert!(e[1].is_none());
        assert_eq!(e[2].unwrap().kind, EventKind::MusicPlayback(true));
        s.bluetooth = radio::BluetoothState::Connected;
        assert_eq!(m.observe(6, s, audio::State::Ready)[2].unwrap().kind, EventKind::MusicPlayback(false));
    }
}
