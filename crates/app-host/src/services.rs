#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug)]
pub struct AudioStatus {
    pub available: bool,
    pub controls_available: bool,
    pub state: &'static str,
    pub sample_rate_hz: u32,
    pub volume: u8,
    pub output_ceiling_db: i16,
    pub muted: bool,
    pub tx_frames: u32,
    pub rx_frames: u32,
    pub underruns: u32,
    pub overruns: u32,
    pub peak: u16,
    pub error: Option<&'static str>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug)]
pub struct CameraStatus {
    pub available: bool,
    pub state: &'static str,
    pub sensor: &'static str,
    pub width: u16,
    pub height: u16,
    pub frames: u32,
    pub dropped: u32,
    pub saved: u32,
    pub error: Option<&'static str>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug)]
pub struct NetworkStatus {
    pub wifi_state: &'static str,
    pub bluetooth_state: &'static str,
    pub ip: Option<[u8; 4]>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug)]
pub struct NativeUiLimits {
    pub objects: usize,
    pub image_bytes: usize,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceSnapshot {
    pub audio: Option<AudioStatus>,
    pub camera: Option<CameraStatus>,
    pub network: Option<NetworkStatus>,
    pub native_ui: Option<NativeUiLimits>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioControl {
    Volume(u8),
    Muted(bool),
}
