//! Connection edges briefly acknowledge link changes. Playback remains active
//! while playing; reactions, errors and battery expressions retain priority.
use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::SensorEvent, crate::companion::behavior::Expression,
};
use {crate::services::audio, crate::services::radio};

/// Link acknowledgements last four seconds, independent of animation loops.
pub const CONNECTION_EXPRESSION_MS: u64 = 4_000;

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
            EventKind::NetworkConnection(value) => (
                &mut self.network,
                value,
                Expression::NetworkConnected,
                Expression::NetworkDisconnected,
            ),
            EventKind::BluetoothConnection(value) => (
                &mut self.bluetooth,
                value,
                Expression::BluetoothConnected,
                Expression::BluetoothDisconnected,
            ),
            EventKind::MusicPlayback(playing) => {
                if playing {
                    self.music_since.get_or_insert(at);
                } else {
                    self.music_since = None;
                }
                return;
            }
            _ => return,
        };
        if *last != Some(connected) {
            *last = Some(connected);
            self.connection = Some((if connected { on } else { off }, at));
        }
    }
    pub fn expression(&self, now_ms: u64) -> Option<(Expression, u64)> {
        self.music_since
            .map(|at| (Expression::MusicPlaying, at))
            .or_else(|| {
                self.connection
                    .filter(|(_, at)| now_ms.saturating_sub(*at) < CONNECTION_EXPRESSION_MS)
            })
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
    pub fn observe(
        &mut self,
        at_ms: u64,
        radio: radio::Snapshot,
        audio: audio::State,
    ) -> [Option<SensorEvent>; 3] {
        let network = if radio.wifi == radio::WifiState::Scanning {
            self.network
        } else {
            radio.wifi == radio::WifiState::Connected
        };
        let bluetooth = matches!(
            radio.bluetooth,
            radio::BluetoothState::Connected | radio::BluetoothState::Streaming
        );
        let music =
            radio.bluetooth == radio::BluetoothState::Streaming || audio == audio::State::Playing;
        let values = [
            (network != self.network).then_some(EventKind::NetworkConnection(network)),
            (bluetooth != self.bluetooth).then_some(EventKind::BluetoothConnection(bluetooth)),
            (music != self.music).then_some(EventKind::MusicPlayback(music)),
        ];
        self.network = network;
        self.bluetooth = bluetooth;
        self.music = music;
        values.map(|kind| {
            kind.map(|kind| SensorEvent {
                at_ms,
                source: EventSource::System,
                kind,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use {crate::companion::widget::FoxControl, crate::Runtime};
    fn event(runtime: &mut Runtime, at: u64, kind: EventKind) {
        runtime.on_sensor(SensorEvent {
            at_ms: at,
            source: EventSource::System,
            kind,
        });
        runtime.tick(at);
    }
    #[test]
    fn native_and_lua_expire_connection_edges_at_four_seconds_without_replaying_them() {
        for lua in [false, true] {
            let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
            if lua {
                runtime
                    .enable_brain(
                        crate::companion::brain::DEFAULT_SOURCE,
                        0,
                        &lua_engine::GLOBAL_ALLOCATOR,
                        None,
                    )
                    .unwrap();
            }
            for (at, kind, expected) in [
                (
                    10,
                    EventKind::NetworkConnection(true),
                    Expression::NetworkConnected,
                ),
                (
                    4_009,
                    EventKind::NetworkConnection(true),
                    Expression::NetworkConnected,
                ),
                (4_010, EventKind::NetworkConnection(true), Expression::Idle),
                (4_020, EventKind::NetworkConnection(true), Expression::Idle),
                (
                    5_000,
                    EventKind::BluetoothConnection(true),
                    Expression::BluetoothConnected,
                ),
                (
                    6_000,
                    EventKind::NetworkConnection(false),
                    Expression::NetworkDisconnected,
                ),
                (
                    9_999,
                    EventKind::NetworkConnection(false),
                    Expression::NetworkDisconnected,
                ),
                (
                    10_000,
                    EventKind::NetworkConnection(false),
                    Expression::Idle,
                ),
                (
                    11_000,
                    EventKind::BluetoothConnection(false),
                    Expression::BluetoothDisconnected,
                ),
                (
                    11_010,
                    EventKind::MusicPlayback(true),
                    Expression::MusicPlaying,
                ),
                (16_000, EventKind::MusicPlayback(false), Expression::Idle),
                (
                    17_000,
                    EventKind::MusicPlayback(true),
                    Expression::MusicPlaying,
                ),
                (
                    18_000,
                    EventKind::BluetoothConnection(true),
                    Expression::MusicPlaying,
                ),
                (
                    18_100,
                    EventKind::MusicPlayback(false),
                    Expression::BluetoothConnected,
                ),
                (
                    22_000,
                    EventKind::BluetoothConnection(true),
                    Expression::Idle,
                ),
                (
                    23_000,
                    EventKind::Activity(Expression::Error),
                    Expression::Error,
                ),
                (
                    23_010,
                    EventKind::NetworkConnection(true),
                    Expression::Error,
                ),
                (
                    28_000,
                    EventKind::Activity(Expression::Idle),
                    Expression::Idle,
                ),
                (29_000, EventKind::BatteryLevel(3), Expression::Dying),
                (
                    29_010,
                    EventKind::BluetoothConnection(false),
                    Expression::Dying,
                ),
                (34_000, EventKind::BatteryLevel(80), Expression::Idle),
            ] {
                event(&mut runtime, at, kind);
                assert_eq!(runtime.expression(), expected, "lua={lua}, at={at}");
            }
            assert_eq!(runtime.brain_active(), lua);
        }
    }
    #[test]
    fn quiet_ticks_expire_each_link_expression_without_another_service_event() {
        for lua in [false, true] {
            for (kind, expression) in [
                (
                    EventKind::NetworkConnection(true),
                    Expression::NetworkConnected,
                ),
                (
                    EventKind::NetworkConnection(false),
                    Expression::NetworkDisconnected,
                ),
                (
                    EventKind::BluetoothConnection(true),
                    Expression::BluetoothConnected,
                ),
                (
                    EventKind::BluetoothConnection(false),
                    Expression::BluetoothDisconnected,
                ),
            ] {
                let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
                if lua {
                    runtime
                        .enable_brain(
                            crate::companion::brain::DEFAULT_SOURCE,
                            0,
                            &lua_engine::GLOBAL_ALLOCATOR,
                            None,
                        )
                        .unwrap();
                }
                event(&mut runtime, 100, kind);
                runtime.tick(4_099);
                assert_eq!(runtime.expression(), expression, "lua={lua}, kind={kind:?}");
                runtime.tick(4_100);
                assert_eq!(
                    runtime.expression(),
                    Expression::Idle,
                    "lua={lua}, kind={kind:?}"
                );
            }
        }
    }
    #[test]
    fn service_monitor_emits_link_edges_and_playback_without_scan_or_boot_disconnects() {
        let mut m = Monitor::default();
        let mut s = radio::Snapshot::new();
        assert!(m
            .observe(0, s, audio::State::Ready)
            .iter()
            .all(Option::is_none));
        s.wifi = radio::WifiState::Connected;
        assert_eq!(
            m.observe(1, s, audio::State::Ready)[0].unwrap().kind,
            EventKind::NetworkConnection(true)
        );
        s.wifi = radio::WifiState::Scanning;
        assert!(m
            .observe(2, s, audio::State::Ready)
            .iter()
            .all(Option::is_none));
        s.wifi = radio::WifiState::Reconnecting;
        assert_eq!(
            m.observe(3, s, audio::State::Ready)[0].unwrap().kind,
            EventKind::NetworkConnection(false)
        );
        s.bluetooth = radio::BluetoothState::Connected;
        assert!(m.observe(4, s, audio::State::Ready)[1].is_some());
        s.bluetooth = radio::BluetoothState::Streaming;
        let e = m.observe(5, s, audio::State::Playing);
        assert!(e[1].is_none());
        assert_eq!(e[2].unwrap().kind, EventKind::MusicPlayback(true));
        s.bluetooth = radio::BluetoothState::Connected;
        assert_eq!(
            m.observe(6, s, audio::State::Ready)[2].unwrap().kind,
            EventKind::MusicPlayback(false)
        );
    }
}
