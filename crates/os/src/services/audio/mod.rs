//! PCM service contracts shared by the codec, local files and Bluetooth audio.
//! Transport callbacks enqueue whole frames; only the audio task touches I2S.
use alloc::{collections::VecDeque, string::String};

pub mod wav;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u8,
}
impl Format {
    pub const SPEECH: Self = Self {
        rate: 16_000,
        channels: 1,
    };
    pub fn new(rate: u32, channels: u8) -> Result<Self, &'static str> {
        if !matches!(rate, 16_000 | 32_000 | 44_100 | 48_000) || !matches!(channels, 1 | 2) {
            return Err("PCM requires 16/32/44.1/48 kHz, mono/stereo, signed 16-bit LE");
        }
        Ok(Self { rate, channels })
    }
    pub const fn frame_bytes(self) -> usize {
        self.channels as usize * 2
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Off,
    Starting,
    Ready,
    Playing,
    Recording,
    Fault,
}
impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Playing => "playing",
            Self::Recording => "recording",
            Self::Fault => "fault",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: State,
    pub rate: u32,
    pub volume: u8,
    pub muted: bool,
    pub tx_frames: u32,
    pub rx_frames: u32,
    pub underruns: u32,
    pub overruns: u32,
    pub peak: u16,
    pub error: Option<&'static str>,
}
impl Snapshot {
    pub const fn new() -> Self {
        Self {
            state: State::Off,
            rate: 0,
            volume: 50,
            muted: false,
            tx_frames: 0,
            rx_frames: 0,
            underruns: 0,
            overruns: 0,
            peak: 0,
            error: None,
        }
    }
}
impl Default for Snapshot {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Status,

    Play(String),
    Record { path: String, ms: u16 },

    Volume(u8),
    Mute(bool),
    Stop,
}
impl Command {
    pub fn parse(verb: &str, args: &[&str]) -> Result<Self, &'static str> {
        let number = |v: &str| v.parse::<u16>().map_err(|_| "invalid audio number");
        let path = |p: &str| {
            if !p.starts_with("/data/")
                || p.split('/').any(|v| matches!(v, "." | ".."))
                || p.len() > 240
                || p.bytes().any(|b| b < 32)
                || !p.ends_with(".wav")
            {
                Err("audio file requires an absolute /data/*.wav path")
            } else {
                Ok(String::from(p))
            }
        };
        Ok(match (verb, args) {
            ("audio-status", []) => Self::Status,
            ("audio-stop", []) => Self::Stop,

            ("audio-volume", [value]) => {
                let value = number(value)?;
                if value > 100 {
                    return Err("volume requires 0..100");
                }
                Self::Volume(value as u8)
            }
            ("audio-mute", ["0"]) => Self::Mute(false),
            ("audio-mute", ["1"]) => Self::Mute(true),
            ("audio-play", [p]) => Self::Play(path(p)?),
            ("audio-record", [p, ms]) => {
                let ms = number(ms)?;
                if !(100..=30_000).contains(&ms) {
                    return Err("record requires 100..30000 ms");
                }
                Self::Record { path: path(p)?, ms }
            }

            _ => return Err("invalid audio command"),
        })
    }
    pub const fn verb(&self) -> &'static str {
        match self {
            Self::Status => "audio-status",

            Self::Play(_) => "audio-play",
            Self::Record { .. } => "audio-record",

            Self::Volume(_) => "audio-volume",
            Self::Mute(_) => "audio-mute",
            Self::Stop => "audio-stop",
        }
    }
}

/// Bluetooth uses 800 ms of capacity at its maximum supported sample rate.
/// Real Wi-Fi coexistence delivered PCM in gaps approaching 300 ms; leave
/// headroom on both sides of the 400 ms startup waterline for those bursts.
pub const BLUETOOTH_QUEUE_FRAMES: usize = 48_000 * 800 / 1000;

pub const fn bluetooth_prefill_frames(format: Format) -> usize {
    format.rate as usize * 400 / 1000
}

/// Stereo frames, bounded independently of callback burst size. No partial frame
/// can enter the queue; a rejected packet never changes the previous contents.
pub struct PcmQueue {
    frames: VecDeque<[i16; 2]>,
    capacity: usize,
}
impl PcmQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity),
            capacity,
        }
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn clear(&mut self) {
        self.frames.clear();
    }
    pub fn push(&mut self, pcm: &[u8], channels: u8) -> Result<usize, &'static str> {
        if !matches!(channels, 1 | 2) || pcm.len() % (usize::from(channels) * 2) != 0 {
            return Err("incomplete PCM frame");
        }
        let count = pcm.len() / (usize::from(channels) * 2);
        if count > self.capacity - self.frames.len() {
            return Err("PCM queue full");
        }
        for frame in pcm.chunks_exact(usize::from(channels) * 2) {
            let left = i16::from_le_bytes([frame[0], frame[1]]);
            let right = if channels == 2 {
                i16::from_le_bytes([frame[2], frame[3]])
            } else {
                left
            };
            // ES8311 has one DAC. Downmix before duplicating so right-only
            // material remains audible and summation cannot overflow.
            let mono = ((i32::from(left) + i32::from(right)) / 2) as i16;
            self.frames.push_back([mono, mono]);
        }
        Ok(count)
    }
    pub fn pop_into(&mut self, out: &mut [u8]) -> usize {
        let mut count = 0;
        for slot in out.chunks_exact_mut(4) {
            let Some(frame) = self.frames.pop_front() else {
                break;
            };
            slot[..2].copy_from_slice(&frame[0].to_le_bytes());
            slot[2..].copy_from_slice(&frame[1].to_le_bytes());
            count += 4;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bluetooth_buffer_survives_300ms_delivery_gaps_and_bursts() {
        fn enqueue(q: &mut PcmQueue, mut frames: usize) {
            let packet = [0u8; 1024 * 4];
            while frames != 0 {
                let count = frames.min(1024);
                assert_eq!(q.push(&packet[..count * 4], 2), Ok(count));
                frames -= count;
            }
        }
        fn consume(q: &mut PcmQueue, mut frames: usize) {
            let mut out = [0u8; 128 * 4];
            while frames != 0 {
                let count = frames.min(128);
                assert_eq!(
                    q.pop_into(&mut out[..count * 4]),
                    count * 4,
                    "producer gap exhausted the playback queue"
                );
                frames -= count;
            }
        }
        for rate in [16_000, 32_000, 44_100, 48_000] {
            let format = Format::new(rate, 2).unwrap();
            let prefill = bluetooth_prefill_frames(format);
            let burst = rate as usize * 300 / 1000;
            for early_burst in [false, true] {
                let mut q = PcmQueue::new(BLUETOOTH_QUEUE_FRAMES);
                enqueue(&mut q, prefill);
                for _ in 0..3 {
                    if early_burst {
                        enqueue(&mut q, burst);
                        consume(&mut q, burst);
                    } else {
                        consume(&mut q, burst);
                        enqueue(&mut q, burst);
                    }
                    assert_eq!(q.len(), prefill);
                }
            }
        }
    }
    #[test]
    fn queue_rejects_overflow_and_partial_frames_atomically() {
        let mut q = PcmQueue::new(2);
        q.push(&[1, 0, 3, 0], 2).unwrap();
        assert!(q.push(&[0; 8], 2).is_err());
        assert!(q.push(&[1, 2, 3], 2).is_err());
        assert_eq!(q.len(), 1);
        let mut out = [0; 8];
        assert_eq!(q.pop_into(&mut out), 4);
        assert_eq!(&out[..4], &[2, 0, 2, 0]);
    }
    #[test]
    fn stereo_downmix_preserves_both_channels_without_clipping() {
        let mut q = PcmQueue::new(3);
        q.push(&[255, 127, 255, 127, 0, 128, 0, 128, 0, 0, 16, 0], 2)
            .unwrap();
        let mut out = [0; 12];
        q.pop_into(&mut out);
        assert_eq!(i16::from_le_bytes([out[0], out[1]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([out[4], out[5]]), i16::MIN);
        assert_eq!(i16::from_le_bytes([out[8], out[9]]), 8);
    }
    #[test]
    fn commands_bound_real_hardware_work_and_paths() {
        assert!(Command::parse("audio-volume", &["101"]).is_err());
        assert!(Command::parse("audio-tone", &["440", "65535"]).is_err());
        assert!(Command::parse("audio-record", &["/data/a.wav", "30001"]).is_err());
        assert!(Command::parse("audio-play", &["/data/../system/a.wav"]).is_err());
        assert!(Command::parse("audio-record", &["/data/audio/test.wav", "1000"]).is_ok());
    }
}

#[cfg(target_os = "none")]
pub(crate) mod task;
