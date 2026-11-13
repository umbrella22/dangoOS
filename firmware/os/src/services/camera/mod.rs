//! Camera commands and consumer state, independent of sensor and DMA backend.
pub mod image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,

    Start,
    Stop,
    Capture,
}
impl Command {
    pub fn parse(verb: &str, args: &[&str]) -> Result<Self, &'static str> {
        if !args.is_empty() {
            return Err("camera commands take no arguments");
        }
        match verb {
            "camera-status" => Ok(Self::Status),

            "camera-start" => Ok(Self::Start),
            "camera-stop" => Ok(Self::Stop),
            "camera-capture" => Ok(Self::Capture),
            _ => Err("unknown camera command"),
        }
    }
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Status => "camera-status",

            Self::Start => "camera-start",
            Self::Stop => "camera-stop",
            Self::Capture => "camera-capture",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Off,
    Starting,
    Preview,
    Fault,
}
impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Starting => "starting",
            Self::Preview => "preview",
            Self::Fault => "fault",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub state: State,
    pub pid: u16,
    pub frames: u32,
    pub dropped: u32,
    pub width: u16,
    pub height: u16,
    pub saved: u32,
    pub error: Option<&'static str>,
}
impl Snapshot {
    pub const fn new() -> Self {
        Self {
            state: State::Off,
            pid: 0,
            frames: 0,
            dropped: 0,
            width: 0,
            height: 0,
            saved: 0,
            error: None,
        }
    }
    pub const fn sensor(self) -> &'static str {
        match self.pid {
            0x364c | 0x3641 => "OV3640",
            0xda4a => "SC101IOT",
            _ => "unknown",
        }
    }
}

#[cfg(target_os = "none")]
pub(crate) mod task;
